"""Load the self-contained frozen guard case corpus."""

def load_case(case, root):
    def sources(files):
        return {path: source if isinstance(source, str) else (root / source['file']).read_text()
                for path, source in files.items()}
    return sources(case['before']), sources(case['after'])

