# Rexeb Dependency Database

This directory holds the curated Debian → Arch package mapping data served to `rexeb update`.

- `mappings.json` — `{version, count, mappings: {debian_name -> {debian_name, arch_name, confidence, source}}}`
- `virtual_packages.json` — `{version, virtual_packages: {virtual_name -> [providers]}}`

Both files are:

1. **Bundled at compile time** via `include_str!("../../db/mappings.json")` in `src/resolver/database.rs`
2. **Served via raw GitHub** at `https://raw.githubusercontent.com/OnionOrbit/rexeb/main/db/mappings.json` (used by `rexeb update --mappings`)
3. **Overlaid on device** by user additions in `~/.local/share/rexeb/db/mappings.json`

To regenerate:

```bash
cargo run --bin generate-mappings  # if added, otherwise:
python3 scripts/generate_mappings.py
```

To enlarge with repo crawls:

```bash
rexeb update --enlarge
```

Keep `count` in sync.
