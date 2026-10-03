#!/usr/bin/env bash
# Sourced by demo.tape. Git writes and preferences stay in a temp workspace.
set -euo pipefail

# The recording must show the astro palette even in a NO_COLOR environment.
unset NO_COLOR
export COLORTERM=truecolor

auri_demo_project=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cargo build --manifest-path "$auri_demo_project/Cargo.toml" --release --locked -q
export AURI_DEMO_DIR
AURI_DEMO_DIR=$(mktemp -d "${TMPDIR:-/tmp}/auri-demo.XXXXXX")
trap 'rm -rf -- "$AURI_DEMO_DIR"' EXIT
export AURI_CONFIG_DIR="$AURI_DEMO_DIR/preferences"
export PATH="$auri_demo_project/target/release:$PATH"
mkdir -p "$AURI_DEMO_DIR/orbit/src" "$AURI_CONFIG_DIR"
cat > "$AURI_CONFIG_DIR/preferences.toml" <<'PREFS'
[layout]
sidebar_view = "explorer"
sidebar_width = 34
history_height = 11

[explorer]
icons = "text"
PREFS

cd "$AURI_DEMO_DIR/orbit"
export PS1='orbit $ '
git init -q -b main
git config user.name "Matheus"
git config user.email "demo@example.com"
git config commit.gpgsign false
git config core.hooksPath "$AURI_DEMO_DIR/no-hooks"
cat > Cargo.toml <<'CARGO'
[package]
name = "orbit"
version = "0.1.0"
edition = "2021"
CARGO
cat > README.md <<'README'
# orbit

A small observatory for your terminal.

- Track the brightest stars in the night sky.
- Keep a shortlist for tonight's observation.
- Print a readable, compact sky report.
README
cat > src/main.rs <<'RUST'
mod settings;

struct Star {
    name: &'static str,
    constellation: &'static str,
    magnitude: f32,
}

fn catalog() -> Vec<Star> {
    vec![
        Star {
            name: "Albireo",
            constellation: "Cygnus",
            magnitude: 3.1,
        },
        Star {
            name: "Vega",
            constellation: "Lyra",
            magnitude: 0.0,
        },
        Star {
            name: "Deneb",
            constellation: "Cygnus",
            magnitude: 1.3,
        },
    ]
}

fn visible_stars(stars: &[Star]) -> Vec<&Star> {
    stars.iter().collect()
}

fn print_report(stars: &[&Star]) {
    println!("Tonight's sky");
    for star in stars {
        println!("{} / {}", star.name, star.constellation);
    }
}

fn main() {
    let stars = catalog();
    let visible = visible_stars(&stars);
    print_report(&visible);
}
RUST
cat > src/settings.rs <<'RUST'
pub const MAGNITUDE_LIMIT: f32 = 4.0;
pub const SHOW_CONSTELLATION: bool = false;
RUST
git add .
GIT_AUTHOR_DATE="2026-09-28T18:00:00Z" GIT_COMMITTER_DATE="2026-09-28T18:00:00Z" \
    git commit -qm "feat: add a night sky catalog"

cat >> README.md <<'README'

## Run

```sh
cargo run
```
README
git add README.md
GIT_AUTHOR_DATE="2026-09-29T18:00:00Z" GIT_COMMITTER_DATE="2026-09-29T18:00:00Z" \
    git commit -qm "docs: explain how to run orbit"

# One staged file, one working diff with separated changes, one new file.
cat > src/settings.rs <<'RUST'
pub const MAGNITUDE_LIMIT: f32 = 3.5;
pub const SHOW_CONSTELLATION: bool = true;
RUST
git add src/settings.rs
python3 - <<'PY'
from pathlib import Path

path = Path("src/main.rs")
source = path.read_text()
source = source.replace(
    "    stars.iter().collect()",
    "    let limit = settings::MAGNITUDE_LIMIT;\n"
    "    stars\n"
    "        .iter()\n"
    "        .filter(|star| star.magnitude <= limit)\n"
    "        .collect()",
)
source = source.replace(
    '    println!("Tonight\'s sky");',
    '    println!("Tonight\'s sky — {} stars", stars.len());',
)
source = source.replace(
    '        println!("{} / {}", star.name, star.constellation);',
    "        if settings::SHOW_CONSTELLATION {\n"
    '            println!("{} / {}", star.name, star.constellation);\n'
    "        } else {\n"
    '            println!("{}", star.name);\n'
    "        }",
)
path.write_text(source)
Path("notes.md").write_text("# Tonight\n\n- Find Albireo in Cygnus.\n- Compare its gold and blue stars.\n")
PY
printf '\nAURI_DEMO_READY\n'
