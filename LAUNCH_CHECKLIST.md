# Checklist de lançamento — v0.1.0

O que falta para lançar o auri, levantado em 2026-09-29. Marque `[x]` ao
concluir. O passo a passo de cada release está em [RELEASING.md](RELEASING.md).

## Já verificado

- [x] Repo `matheusz-nied/auri-tui` público; CI (fmt + clippy + testes,
  Linux e macOS) verde no commit `1028c35`.
- [x] Landing page no ar: https://matheusz-nied.github.io/auri-tui/
- [x] `cargo publish --dry-run` passa (38 arquivos, ~104 KiB, compila a
  partir do pacote).
- [x] `dist plan` gera os binários de `aarch64/x86_64-apple-darwin`,
  `aarch64/x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl` e o
  instalador shell.
- [x] Nome `auri-tui` livre no crates.io.
- [x] Bugs 1–9 do [BACKLOG.md](BACKLOG.md) corrigidos e commitados.

## Bloqueia o lançamento

- [ ] **Gerar o GIF de demo (ou tirar a imagem do README).** O README
  referencia `demo/demo.gif`, que não existe: imagem quebrada no GitHub e no
  crates.io.
  ```sh
  brew install vhs
  vhs demo/demo.tape   # rodar num repo com algumas mudanças não commitadas
  ```
- [ ] **Datar a versão no `CHANGELOG.md`.** Trocar
  `## [0.1.0] - Unreleased` pela data do lançamento (ex.:
  `## [0.1.0] - 2026-09-30`). O dist usa essa seção como nota da release.
- [ ] **Criar e enviar a tag `v0.1.0`.** Dispara o workflow `Release`, que
  gera os binários e a release no GitHub. Até lá, o instalador anunciado no
  README e na landing page responde 404.
  ```sh
  git tag v0.1.0 && git push --tags
  ```
- [ ] **Publicar no crates.io** depois que a release sair (`cargo install
  auri-tui` só funciona depois disso).
  ```sh
  cargo login      # uma vez, com token do crates.io
  cargo publish
  ```
- [ ] **Smoke test do instalador** numa máquina limpa:
  ```sh
  curl -LsSf https://github.com/matheusz-nied/auri-tui/releases/latest/download/auri-tui-installer.sh | sh
  auri --version
  ```

## Recomendado (não bloqueia)

- [ ] **Descrição e tópicos do repo no GitHub** — hoje vazios. Sugestão de
  descrição: "Browse files and git diffs side by side, right in your
  terminal"; tópicos: `git`, `tui`, `terminal`, `diff`, `rust`, `ratatui`.
- [ ] **Confirmar o modelo padrão da IA.** O padrão é `codex` com
  `codex_model = "gpt-6-luna"` (`src/ai/mod.rs`). Se o nome não for válido,
  o `Ctrl-G` falha para todo mundo com a configuração padrão.
- [ ] **Mostrar o GIF na landing page.** O workflow de Pages já copia
  `demo/demo.gif` para o site, mas `site/index.html` não usa nenhuma imagem.
- [ ] **Documentar limitações conhecidas** no README, ou resolver na 0.2:
  - durante um merge em que todas as resoluções ficam iguais ao último
    commit, não sobra nada staged e o commit é bloqueado com
    "Nothing staged", embora um commit de merge fosse válido;
  - sem suporte a Windows (os binários são só macOS e Linux).
