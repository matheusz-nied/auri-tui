# Backlog de melhorias

Problemas e melhorias levantados na análise do estado do repo (2026-09-28).
Marque `[x]` ao resolver e referencie o commit/PR ao lado.

## Bugs e comportamento incorreto

- [x] **1. Ctrl+tecla insere texto no commit** — com o input focado, `Ctrl+A`
  insere `a`. `app.rs:227` repassa tudo que não é Tab/Esc/Ctrl+C e
  `commit_input.rs:26` aceita qualquer `Char` sem checar modificadores.
- [x] **2. Mensagem de commit longa some** — o input não tem scroll horizontal;
  o texto é cortado e o cursor desaparece após a largura do painel
  (`commit_input.rs:92`).
- [x] **3. Caracteres largos desalinham a UI** — CJK/emoji usam
  `chars().count()` como largura em vez da largura de exibição (unicode-width).
  Afeta padding do diff, cursor do commit e alinhamento da letra de status na
  lista de mudanças.
- [x] **4. Mensagens do status bar nunca somem** — "Committed" e erros ficam
  para sempre (`app.rs:178-183`). Adicionar expiração.
- [x] **5. Conflitos de merge não tratados** — `UU` aparece duplicado em
  Staged e Changes, e o código `U` é usado tanto para untracked quanto para
  unmerged. Precisa de seção/indicador próprio para conflitos.
- [x] **6. Clicar no arquivo já selecionado recarrega o diff** e reseta o
  scroll para o topo.
- [x] **7. Commit com hook interativo / GPG** (pinentry no tty) pode corromper
  a TUI, pois `git commit` roda com o terminal em raw mode.
- [x] **8. Paths longos na lista de mudanças** não são truncados com `…` e a
  letra de status é cortada.
- [x] **9. Docstring errada** em `resolve_toplevel` — diz que retorna `None`
  mas retorna `Err` (`cli.rs:138`).

## Performance

- [x] **10. Git síncrono na thread da UI** — a cada 2 s roda `status` +
  `branch` + diff completo (`-U100000`) do arquivo selecionado. Em repo ou
  arquivo grande a UI trava. Mover para um worker com canal que devolve
  `Action`s. *(problema estrutural mais relevante)*
- [x] **11. Polling fixo** — usar watcher de filesystem (crate `notify`) para
  refresh instantâneo e sem custo quando nada muda.
  *Feito: `watch.rs` (worktree + `HEAD`/`index`/`refs` do .git, debounce de
  150 ms); o tick virou reserva de 30 s, ou 2 s se o watcher não subir.*
- [x] **12. `DiffDoc` clonado várias vezes por carga** (`last_diff`, broadcast,
  `set_doc`). Considerar `Arc<DiffDoc>`.

## Arquitetura

- [x] **13. Estado de seleção duplicado** — `App` guarda `selected` e
  `last_diff`, enquanto `Changes` e `DiffView` guardam a própria seleção. O
  hack em `app.rs:125` ("já tem `SelectFile` na fila?") depende da ordem da
  fila e é frágil.
  *Feito com o worker do git: o hack saiu; resultados de diff são casados
  por identidade (`DiffSource`) em `App::accept`. `Changes`/`DiffView`
  seguem com o próprio estado de seleção de UI.*
- [ ] **14. Layout fixo em `App::render`** — componentes são plugáveis, mas o
  layout não; todo painel novo exige mexer no `App`, que tende a virar um
  "god object".
- [x] **15. Atalhos globais fixos num `match`** (`app.rs:223-282`) — sem
  keymap, e o help do status bar é uma string manual. Gerar help a partir do
  keymap.
- [ ] **16. `App` sem testes** — o doc comment promete "mock backend", mas não
  existe. Criar um `FakeGit` e testar `dispatch`/`execute`.

## Repositório e tooling

- [ ] **17. Diretório `.mimocode/`** (plugin de outra ferramenta) na raiz e
  não ignorado — adicionar ao `.gitignore`.
- [x] **18. Sem README, sem CI e sem o primeiro commit.**
- [ ] **19. Versões fixadas com `=` no `Cargo.toml`** (incomum para binário,
  trava updates) e edition 2021 (poderia ser 2024).

## Funcionalidades que faltam

- [x] Descartar mudanças de um arquivo
- [ ] Stage/unstage por hunk ou linha
- [x] Abrir arquivo no `$EDITOR`
- [x] Commit multilinha e amend
- [ ] Push / pull
- [ ] Scroll horizontal com Shift+roda do mouse
- [x] Redimensionar painéis com o mouse
- [x] Árvore de arquivos
- [x] Histórico / log

## Ordem sugerida

1. Higiene: itens 17, 18, 19.
2. Git fora da UI thread: item 10 (simplifica o 13).
3. `FakeGit` e testes do `App`: item 16 — rede de segurança para refatorar.
4. Bugs de UX pequenos: itens 1, 2, 3, 4, 6, 8, 9.
5. Features, na ordem de valor.
