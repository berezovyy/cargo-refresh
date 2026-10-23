//! Manual suspended-future safety matrix bridge through the production extractor and agent protocol.
use anyhow::{Context, Result, ensure};
use cargo_refresh::{agent, elf::Image, extractor::ManifestExtractor, manifest::State};
fn main() -> Result<()> {
    let a: Vec<_> = std::env::args().skip(1).collect();
    ensure!(
        a.len() == 4,
        "usage: async_patch BASELINE CANDIDATE MANIFEST SOCKET"
    );
    let mut state = State::default();
    state.update(&std::fs::read(&a[2])?)?;
    let candidate = Image::parse(std::fs::read(&a[1])?, false)?;
    let mut extractor = ManifestExtractor::new(&a[0], false)?;
    let patch = extractor.extract(&state, &candidate)?;
    let mut client = agent::Client::new(&a[3]);
    let reply = client
        .apply(&patch.records)
        .context("apply async safety patch")?;
    ensure!(reply.ok, "agent refused: {} {}", reply.code, reply.detail());
    extractor.commit(patch)?;
    println!("{}", serde_json::to_string(&reply)?);
    Ok(())
}
