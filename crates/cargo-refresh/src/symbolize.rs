//! Per-process, append-only symbols and bounded, byte-preserving stderr filtering.
use anyhow::{Context, Result};
use object::{Object, ObjectSymbol, SymbolKind};
use refresh_proto::manifest::Symbol;
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Write},
    path::Path,
};

#[derive(Debug)]
struct Entry {
    end: u64,
    name: String,
    file: String,
    line: u32,
    generation: Option<u64>,
}
#[derive(Default, Debug)]
pub struct SymbolIndex {
    entries: BTreeMap<u64, Entry>,
}
impl SymbolIndex {
    /// Read only the immutable run snapshot, once per application process.
    pub fn baseline(path: &Path, load_base: u64) -> Result<Self> {
        let file = fs::File::open(path)?;
        // SAFETY: Supervisor owns this immutable run snapshot. It is never
        // overwritten or truncated, including while the process is stopped.
        let bytes = unsafe { memmap2::Mmap::map(&file)? };
        let image = object::File::parse(&*bytes)?;
        let mut index = Self::default();
        for symbol in image.symbols().chain(image.dynamic_symbols()) {
            if symbol.kind() != SymbolKind::Text || symbol.is_undefined() || symbol.size() == 0 {
                continue;
            }
            let Ok(name) = symbol.name() else { continue };
            let name = format!("{:#}", rustc_demangle::demangle(name));
            index.insert(
                load_base,
                &Symbol {
                    addr: symbol.address(),
                    size: symbol.size(),
                    name,
                    file: String::new(),
                    first_line: 0,
                    line_table: Vec::new(),
                },
                None,
            );
        }
        Ok(index)
    }
    fn insert(&mut self, base: u64, symbol: &Symbol, generation: Option<u64>) {
        let Some(start) = base.checked_add(symbol.addr) else {
            return;
        };
        let Some(end) = start.checked_add(symbol.size) else {
            return;
        };
        if symbol.size == 0 || symbol.name.is_empty() {
            return;
        }
        // Deterministic first alias wins. Never replace symbols of older code.
        self.entries.entry(start).or_insert_with(|| Entry {
            end,
            name: symbol.name.replace(['\r', '\n'], " "),
            file: symbol.file.replace(['\r', '\n'], " "),
            line: symbol.first_line,
            generation,
        });
    }
    pub fn add_generation(
        &mut self,
        load_base: u64,
        generation: u64,
        symbols: &[Symbol],
    ) -> Vec<u64> {
        let mut inserted = Vec::new();
        for symbol in symbols {
            let Some(address) = load_base.checked_add(symbol.addr) else {
                continue;
            };
            if self.entries.contains_key(&address) {
                continue;
            }
            self.insert(load_base, symbol, Some(generation));
            if self.entries.contains_key(&address) {
                inserted.push(address);
            }
        }
        inserted
    }
    pub fn discard(&mut self, inserted: &[u64]) {
        for address in inserted {
            self.entries.remove(address);
        }
    }
    pub fn rewrite(&self, line: &[u8]) -> Option<Vec<u8>> {
        let text = std::str::from_utf8(line).ok()?;
        let (prefix, address) = frame(text)?;
        // Backtrace IPs are return addresses. Match the call instruction rather
        // than accidentally selecting the next function at an exclusive end.
        let address = address.checked_sub(1)?;
        let (_, entry) = self.entries.range(..=address).next_back()?;
        if address >= entry.end {
            return None;
        }
        let mut output = format!("{prefix}at {}", entry.name);
        if !entry.file.is_empty() && entry.line != 0 {
            output.push_str(&format!("  {}:{}", entry.file, entry.line));
        }
        if let Some(generation) = entry.generation {
            output.push_str(&format!("   [patched, gen {generation}]"));
        }
        if text.ends_with("\r\n") {
            output.push_str("\r\n");
        } else if text.ends_with('\n') {
            output.push('\n');
        }
        Some(output.into_bytes())
    }
}

/// Only numbered std frames, or the address-bearing `at <unknown>` form.
/// Named frames and arbitrary application messages are never changed.
fn frame(text: &str) -> Option<(&str, u64)> {
    let body = text.trim_end_matches(['\r', '\n']);
    let trimmed = body.trim_start_matches(' ');
    let indent = body.len() - trimmed.len();
    let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    let (prefix, rest) = if digits > 0 && trimmed.get(digits..)?.starts_with(": ") {
        let pos = indent + digits + 2;
        (&body[..pos], body[pos..].trim())
    } else if trimmed.starts_with("at ") {
        (&body[..indent], trimmed.trim_start_matches("at ").trim())
    } else {
        return None;
    };
    let (address, symbol) = if let Some(address) = rest.strip_prefix("<unknown>") {
        (address.trim(), "<unknown>")
    } else {
        let end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
        (
            &rest[..end],
            rest[end..].trim().trim_start_matches('-').trim(),
        )
    };
    if !matches!(symbol, "" | "<unknown>") {
        return None;
    }
    let address = address.strip_prefix("0x")?;
    if address.is_empty() || !address.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some((prefix, u64::from_str_radix(address, 16).ok()?))
}

/// Hold only potential frame prefixes; ordinary bytes are written immediately.
/// The pump calls `flush_partial` after 25 ms without a completed candidate.
#[derive(Default)]
pub struct LineFilter {
    pending: Vec<u8>,
    passthrough: bool,
}
impl LineFilter {
    pub fn push(
        &mut self,
        bytes: &[u8],
        index: &SymbolIndex,
        output: &mut impl Write,
    ) -> io::Result<()> {
        for chunk in bytes.split_inclusive(|b| *b == b'\n') {
            let ended = chunk.ends_with(b"\n");
            if self.passthrough {
                output.write_all(chunk)?;
                if ended {
                    self.passthrough = false;
                }
                continue;
            }
            self.pending.extend_from_slice(chunk);
            if ended {
                if let Some(rewritten) = index.rewrite(&self.pending) {
                    output.write_all(&rewritten)?;
                } else {
                    output.write_all(&self.pending)?;
                }
                self.pending.clear();
            } else if self.pending.len() >= 8192 || !potential_frame(&self.pending) {
                self.flush_partial(output)?;
            }
        }
        output.flush()
    }
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
    pub fn flush_partial(&mut self, output: &mut impl Write) -> io::Result<()> {
        if !self.pending.is_empty() {
            output.write_all(&self.pending)?;
            self.pending.clear();
            self.passthrough = true;
        }
        output.flush()
    }
}
fn potential_frame(bytes: &[u8]) -> bool {
    let rest = bytes
        .iter()
        .position(|b| *b != b' ')
        .map_or(&b""[..], |n| &bytes[n..]);
    if rest.is_empty() {
        return true;
    }
    if rest[0].is_ascii_digit() {
        let n = rest.iter().take_while(|b| b.is_ascii_digit()).count();
        return n == rest.len() || rest[n] == b':';
    }
    b"at ".starts_with(rest) || rest.starts_with(b"at ")
}

/// Main ELF's load bias, including ET_EXEC (bias zero). Match the run inode;
/// mapping offsets alone are insufficient for promoted/nonstandard PT_LOADs.
pub fn load_base(pid: u32, executable: &Path) -> Result<u64> {
    use object::{
        LittleEndian,
        read::elf::{FileHeader, ProgramHeader},
    };
    use std::os::unix::fs::MetadataExt;
    let file = fs::File::open(executable)?;
    // SAFETY: this is the supervisor's immutable executable snapshot.
    let bytes = unsafe { memmap2::Mmap::map(&file)? };
    let header = object::elf::FileHeader64::<LittleEndian>::parse(&*bytes)?;
    let programs = header.program_headers(LittleEndian, &*bytes)?;
    let metadata = fs::metadata(executable)?;
    let inode = metadata.ino();
    let device = format!(
        "{:02x}:{:02x}",
        libc::major(metadata.dev()),
        libc::minor(metadata.dev())
    );
    for line in fs::read_to_string(format!("/proc/{pid}/maps"))?.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 5 || fields[3] != device || fields[4].parse::<u64>().ok() != Some(inode) {
            continue;
        }
        let start = u64::from_str_radix(fields[0].split('-').next().unwrap(), 16)?;
        let offset = u64::from_str_radix(fields[2], 16)?;
        for ph in programs {
            if ph.p_type(LittleEndian) == object::elf::PT_LOAD
                && ph.p_offset(LittleEndian) & !4095 == offset
                && let Some(base) = start.checked_sub(ph.p_vaddr(LittleEndian) & !4095)
            {
                return Ok(base);
            }
        }
    }
    None.context("application ELF load bias unavailable")
}

/// One owned pump per process. A timeout flushes incomplete candidate lines;
/// shutdown drains the pipe without waiting for descendants that inherited it.
pub struct StderrPump {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    task: std::thread::JoinHandle<()>,
}
impl StderrPump {
    pub fn start(
        pipe: std::process::ChildStderr,
        symbols: std::sync::Arc<std::sync::RwLock<SymbolIndex>>,
    ) -> Result<Self> {
        Self::start_to(pipe, symbols, io::stderr())
    }
    fn start_to(
        mut pipe: std::process::ChildStderr,
        symbols: std::sync::Arc<std::sync::RwLock<SymbolIndex>>,
        mut output: impl Write + Send + 'static,
    ) -> Result<Self> {
        use std::{
            io::Read,
            os::fd::AsRawFd,
            sync::{
                Arc,
                atomic::{AtomicBool, Ordering},
            },
            time::{Duration, Instant},
        };
        let fd = pipe.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        anyhow::ensure!(
            flags >= 0 && unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == 0,
            "configure application stderr pipe"
        );
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let task = std::thread::Builder::new()
            .name("refresh-stderr".into())
            .spawn(move || {
                let mut filter = LineFilter::default();
                let mut buffer = [0; 8192];
                let mut pending_since = None;
                let mut stopping_since = None;
                loop {
                    if stopping.load(Ordering::Relaxed) {
                        let since = stopping_since.get_or_insert_with(Instant::now);
                        if since.elapsed() >= Duration::from_millis(100) {
                            break;
                        }
                    }
                    let mut poll = libc::pollfd {
                        fd,
                        events: libc::POLLIN,
                        revents: 0,
                    };
                    unsafe {
                        libc::poll(&mut poll, 1, 10);
                    }
                    let result = match pipe.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => {
                            // Do not hold the index lock across terminal/pipe
                            // I/O: a slow stderr consumer must not stall APPLY.
                            let mut rendered = Vec::with_capacity(n);
                            let result = {
                                let index = symbols.read().unwrap();
                                filter.push(&buffer[..n], &index, &mut rendered)
                            };
                            result.and_then(|()| output.write_all(&rendered))
                        }
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(()),
                        Err(_) => break,
                    };
                    if result.is_err() {
                        break;
                    }
                    if filter.has_pending() {
                        let since = pending_since.get_or_insert_with(Instant::now);
                        if since.elapsed() >= Duration::from_millis(25) {
                            if filter.flush_partial(&mut output).is_err() {
                                break;
                            }
                            pending_since = None;
                        }
                    } else {
                        pending_since = None;
                    }
                    if stopping.load(Ordering::Relaxed) && poll.revents & libc::POLLIN == 0 {
                        break;
                    }
                }
                let _ = filter.flush_partial(&mut output);
            })?;
        Ok(Self { stop, task })
    }
    pub fn stop(self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = self.task.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn symbol(addr: u64, line: u32) -> Symbol {
        Symbol {
            addr,
            size: 32,
            name: "server::panic_here".into(),
            file: "src/main.rs".into(),
            first_line: line,
            line_table: vec![],
        }
    }
    fn index() -> SymbolIndex {
        let mut index = SymbolIndex::default();
        for generation in 1..=3 {
            index.add_generation(
                0x5000,
                generation,
                &[symbol(generation * 0x100, generation as u32 * 10)],
            );
        }
        index
    }
    #[test]
    fn full_short_and_missing_symbol() {
        let index = index();
        for line in [
            "   4: 0x5308 - <unknown>\n",
            "   4: 0x5308\n",
            "   4: <unknown> 0x5308\n",
            "   4: 0x5308 -\n",
        ] {
            assert_eq!(
                index.rewrite(line.as_bytes()).unwrap(),
                b"   4: at server::panic_here  src/main.rs:30   [patched, gen 3]\n"
            );
        }
        assert_eq!(
            index.rewrite(b"  at <unknown> 0x5208\r\n").unwrap(),
            b"  at server::panic_here  src/main.rs:20   [patched, gen 2]\r\n"
        );
        assert!(index.rewrite(b"   4: <unknown>\n").is_none());
        assert!(index.rewrite(b"   4: 0x5308 - named::function\n").is_none());
    }
    #[test]
    fn retains_generations_and_rebases_without_scanning_history() {
        let mut index = index();
        index.add_generation(0x5000, 4, &[symbol(0x100, 99)]);
        assert!(
            String::from_utf8(index.rewrite(b" 1: 0x5108 - <unknown>\n").unwrap())
                .unwrap()
                .contains("src/main.rs:10   [patched, gen 1]")
        );
        // Return IP at an exclusive end still belongs to the previous function.
        assert!(index.rewrite(b" 1: 0x5320 - <unknown>\n").is_some());
        assert!(index.rewrite(b" 1: 0x5321 - <unknown>\n").is_none());
        index.add_generation(u64::MAX, 9, &[symbol(8, 0)]);
        assert!(index.rewrite(b" 1: 0x0 - <unknown>\n").is_none());
    }
    #[test]
    fn refused_symbols_can_be_reused_without_stale_lines() {
        let mut index = index();
        let staged = index.add_generation(0x5000, 4, &[symbol(0x400, 40), symbol(0x100, 99)]);
        index.discard(&staged);
        assert!(index.rewrite(b" 1: 0x5408 - <unknown>\n").is_none());
        assert!(index.rewrite(b" 1: 0x5108 - <unknown>\n").is_some());
        index.add_generation(0x5000, 5, &[symbol(0x400, 50)]);
        assert_eq!(
            index.rewrite(b" 1: 0x5408 - <unknown>\n").unwrap(),
            b" 1: at server::panic_here  src/main.rs:50   [patched, gen 5]\n"
        );
    }
    #[test]
    fn no_debug_info_still_has_name_and_generation() {
        let mut symbol = symbol(0x100, 0);
        symbol.file.clear();
        let mut index = SymbolIndex::default();
        index.add_generation(0, 7, &[symbol]);
        assert_eq!(
            index.rewrite(b" 3: 0x108 - <unknown>\n").unwrap(),
            b" 3: at server::panic_here   [patched, gen 7]\n"
        );
    }
    #[test]
    fn interleaved_bytes_partial_reads_and_unrelated_stderr() {
        let input = b"app: \xff\0\r\n   4: 0x5308 - <unknown>\nnoise 0x5308 <unknown>\n   5: 0x9999 - <unknown>\nend";
        let expected = b"app: \xff\0\r\n   4: at server::panic_here  src/main.rs:30   [patched, gen 3]\nnoise 0x5308 <unknown>\n   5: 0x9999 - <unknown>\nend";
        for chunk_size in 1..=input.len() {
            let mut output = vec![];
            let mut filter = LineFilter::default();
            for chunk in input.chunks(chunk_size) {
                filter.push(chunk, &index(), &mut output).unwrap();
            }
            filter.flush_partial(&mut output).unwrap();
            assert_eq!(output, expected, "chunk {chunk_size}");
        }
    }
    #[test]
    fn ordinary_fragments_immediate_candidates_bounded_and_timeout_passthrough() {
        let mut filter = LineFilter::default();
        let mut output = vec![];
        filter
            .push(b"normal partial", &index(), &mut output)
            .unwrap();
        assert_eq!(output, b"normal partial");
        filter.push(b"\n  4: 0x53", &index(), &mut output).unwrap();
        assert!(filter.has_pending());
        filter.flush_partial(&mut output).unwrap();
        filter
            .push(b"08 - <unknown>\n", &index(), &mut output)
            .unwrap();
        assert_eq!(output, b"normal partial\n  4: 0x5308 - <unknown>\n");
        filter
            .push(&vec![b' '; 9000], &index(), &mut output)
            .unwrap();
        assert!(!filter.has_pending());
    }
    #[test]
    fn unrelated_malformed_and_named_frames_byte_identical() {
        let bytes = b"message: <unknown> 0x5308\n42: not a frame\n  2: 0x5308 - real::name\n  2: 0xno - <unknown>\n  2: 0x5308 trailing\n\xff\n";
        let mut output = vec![];
        let mut filter = LineFilter::default();
        filter.push(bytes, &index(), &mut output).unwrap();
        assert_eq!(output, bytes);
    }
    #[derive(Clone, Default)]
    struct Capture(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn idle_partial_frame_flushes_before_eof_and_keeps_tail_bytes() {
        use std::{
            process::{Command, Stdio},
            time::{Duration, Instant},
        };
        let mut child = Command::new("/bin/sh")
            .args([
                "-c",
                "printf ' 4: 0x53' >&2; sleep .5; printf '08 - <unknown>\\nnormal\\377' >&2",
            ])
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let capture = Capture::default();
        let pump = StderrPump::start_to(
            child.stderr.take().unwrap(),
            std::sync::Arc::new(std::sync::RwLock::new(index())),
            capture.clone(),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while capture.0.lock().unwrap().is_empty() {
            assert!(
                Instant::now() < deadline,
                "partial stderr buffered indefinitely"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            child.try_wait().unwrap().is_none(),
            "partial frame waited for EOF"
        );
        assert!(child.wait().unwrap().success());
        pump.stop();
        assert_eq!(
            *capture.0.lock().unwrap(),
            b" 4: 0x5308 - <unknown>\nnormal\xff"
        );
    }
    #[test]
    fn pump_stop_does_not_wait_for_open_writer() {
        use std::{
            os::unix::process::CommandExt,
            process::{Command, Stdio},
            time::{Duration, Instant},
        };
        let mut child = Command::new("/bin/sh")
            .args(["-c", "printf 'ordinary' >&2; sleep 30"])
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let capture = Capture::default();
        let pump = StderrPump::start_to(
            child.stderr.take().unwrap(),
            Default::default(),
            capture.clone(),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while capture.0.lock().unwrap().is_empty() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        pump.stop();
        assert!(child.try_wait().unwrap().is_none());
        crate::supervisor::terminate(&mut child, Duration::from_millis(10));
        assert_eq!(*capture.0.lock().unwrap(), b"ordinary");
    }
    #[test]
    fn indexes_immutable_elf_and_real_load_bias() {
        let exe = std::env::current_exe().unwrap();
        let bias = load_base(std::process::id(), &exe).unwrap();
        let index = SymbolIndex::baseline(&exe, bias).unwrap();
        let address = indexes_immutable_elf_and_real_load_bias as *const () as u64 + 1;
        let line = format!("  1: {address:#x} - <unknown>\n");
        let named = String::from_utf8(index.rewrite(line.as_bytes()).unwrap()).unwrap();
        assert!(
            named.contains("symbolize::tests::indexes_immutable_elf_and_real_load_bias"),
            "{named}"
        );
        assert!(!named.contains("[patched"));
    }
}
