Termine por mim o trabalho abaixo no repositório /Users/erickpatrickbarcelos/codes/polterware-workspace/dispatch (Rust core + app GPUI + plugin do OpenCode Desktop). Leia primeiro o AGENTS.md (é longo; os trechos que importam: parágrafo do plugin do OpenCode, sidebar, run page, bots/avatars, "Verification").

## Regras
- Responda em português. Mudanças pequenas e verificadas; mostre os testes que rodou.
- Delegue implementação com o próprio Dispatch (`dispatch run --worktree-mode worktree ...`, brief autocontido, conferir o diff e testes antes de `dispatch merge`). ATENÇÃO: o modelo deepseek/deepseek-flash (agente opencode) está SEM SALDO ("Insufficient Balance"). Não use DeepSeek; use `--tier normal` (ou um modelo barato de codex/cursor/claude) e confirme com `dispatch models`.
- Nunca commitar/stashar trabalho alheio sem eu mandar. Antes de mesclar, `git status`; se `dispatch merge` recusar por alterações não commitadas ou der conflito, resolva à mão (nos AGENTS.md os conflitos costumam ser só frases a juntar, manter os dois lados).
- Não dar push. Rodar `bash tooling/verify.sh` ao final (ou, se lento, cargo test --workspace, cargo clippy --workspace --all-targets, plugin tests, typecheck e check).
- Worktrees: ao final, `dispatch worktrees` e `--prune` só do que está "can go". A run 95c2b859ff61 (codex/gpt-6-luna, +16/-38, 4 arquivos) é de outra tarefa e NÃO está na main: pergunte-me antes de mesclar ou descartar.

## O que JÁ está feito e mesclado na main (não refazer)
1. Plugin OpenCode: ao clicar numa run no painel Dispatch abre uma tela de log dentro do painel (Back, bot, título, status, "Open in Dispatch", log monospace, poll de 2 s enquanto ativa, segue o fim só se o leitor está no fim). Método RPC `log` em crates/dispatch-core/src/setup/opencode/{rpc.js,server.js}, GUI em plugins/opencode-gui/src/gui.tsx.
2. Check "Delegate" (force) ao lado da pill Dispatch: RPC `force`/`setForce`/evento `forced`; por conversa (storage `force/<root session>`); quando ligado o hook `context` acrescenta instrução FORCED e remove as ferramentas edit/write/patch/apply_patch/multiedit/bash/shell.
3. Seletor de agente e modelo quando o Delegate está ligado (pill "Agent": Auto ou agente+modelo, RPC `targets`, `--agent`/`--model` forçados no dispatch_run).
4. Comando `dispatch events <id> [--tail-bytes n] [--before-seq n --before-offset n] [--json]` (snapshot do log de uma run, inclusive RUNNING; ReadRunEvents sincroniza o agent.log).
5. Limpeza das worktrees extras.

## O que FALTA (na ordem)
A. Log ao vivo no plugin. O método `log` do server.js ainda lê `dispatch result <id> --section output`, vazio até a run acabar: o painel mostra "Waiting for output…" para sempre numa run ativa. Trocar para `dispatch events <id> --json --tail-bytes 262144` (texto como a saída humana do CLI: `[kind] text`, sem ANSI, sem `started`, cap 2000 linhas/256 KB, devolver status), com fallback para `result --section output` se o binário instalado não conhecer `events`. Manter o poll e o follow-the-end. Atualizar plugin.test.mjs (fake dispatch com `events` e o fallback), rebuild `pnpm --dir plugins/opencode-gui build`, `node --test crates/dispatch-core/src/setup/opencode/plugin.test.mjs`, `pnpm --dir plugins/opencode-gui typecheck` e `check`. Uma tentativa parcial existe, NÃO mesclada, na worktree .dispatch/worktrees/ac44bdbb88f177b53f428d14f2f1b40e (branch dispatch/ac44bdbb88f177b53f428d14f2f1b40e, 5 arquivos modificados sem commit, run falhou por falta de saldo): pode aproveitar após revisar, ou refazer.

B. Dialog "Usage" no APP DISPATCH (não no OpenCode): item "Usage" no menu de contexto (botão direito e "…") da conversa no sidebar, entre "Rename…" e "Delete conversation" (app/src/views/sidebar.rs ~926-950). Abre dialog (mesmo padrão do dialog de modelo da Intelligence / Rename: composer_card em modal_layer, Close/Escape/scrim fecha, seções com IconTile) com:
   - "Dispatch runs": uma linha por agente+modelo usados nas runs da conversa (conversation_id, follow-ups incluídos, runs apagadas fora): bot do agente, runs, tokens (input/cache/output) e custo equivalente de API (custo parcial/desconhecido marcado como na página Spend, `Usage::cost_partial`), com total. Fonte: `usage_json` das runs.
   - "Calling agent": o agente do harness que invocou o dispatch (`caller_agent` + sessão no `conversation_id`, ex. `claude-code:<uuid>`, `opencode:ses_...`, `codex:...`): medir o uso dessa sessão lendo o transcript local, no core, via novo RPC `DispatchService.GetConversationUsage(conversation_id)` (proto em schemas/dispatch/v1/dispatch.proto; handler em crates/dispatch-core/src/server/; leitor em novo módulo, ex. crates/dispatch-core/src/usage/caller.rs; ligar em server/routes.rs, coreclient/services.rs, app/src/client.rs). Claude Code: ~/.claude/projects/*/<uuid>.jsonl, somar `message.usage` por `message.model` (dedupe por message id/requestId, streaming repete), preço do catálogo models.dev que o Dispatch já usa. Codex: ~/.codex/sessions/**/rollout-*<thread id>*.jsonl, eventos token_count. OpenCode: ver onde guarda as sessões (~/.local/share/opencode; sqlite/storage), ler só leitura se for confiável, senão "Not measured". Outros: "Not measured for <agent>". Só números e nomes de modelo; validar o id da sessão contra ^[A-Za-z0-9_-]{1,128}$; nunca ler mais que a sessão, nunca logar texto de mensagens; respeitar HOME de teste.
   - Total geral (runs + agente chamador) com a nota "preço equivalente de API, não o que a assinatura cobra".
   - CLI: `dispatch usage --conversation <id> [--json]` com os mesmos números.
   - Testes: unit dos parsers (fixtures jsonl com id duplicado, modelos mistos, linha malformada), e2e_cli com agente ACP falso e HOME falso com transcript Claude de fixture, app/tests/client_e2e.rs para o RPC, ui_flows abrindo o dialog pelo menu. AGENTS.md: uma frase; atualizar o texto de uso da CLI.

C. Paginação das runs por section no sidebar do app: cada grupo mostra 5 linhas, depois "Show 5 more · N hidden" revela mais 5 por clique (5, 10, 15, ...) até acabar (aí pode oferecer "Show less"). Vale para todos os agrupamentos do Display (Conversation, Repository, Date, Status, Agent, None) e para seções customizadas. A unidade é a linha do sidebar (um chat de follow-ups é uma linha, `sidebar_runs`). Linhas que pedem atenção ficam sempre visíveis além do limite: com ponto de atualização não vista (`has_unseen_update`), em execução, e a run aberta; manter a ordem do Display. Quantas linhas cada grupo mostra é guardado neste computador como os grupos recolhidos; "Reset to defaults" volta a 5. Só o app (sem core, proto ou plugin). Testes: unit da função pura de paginação (linhas, limite, fixas -> visíveis, escondidas, próximo limite) e ui_flows com >12 runs num grupo: vê 5, clica Show more duas vezes e vê 15 (usar `debug_selector`). Uma frase no AGENTS.md (parágrafo do sidebar).

D. NOVO — o rosto dos bots deve refletir o estado da execução. Hoje o bot de cada run (bot-avatars: crates do repo `bot-avatar` na raiz do workspace, usado no app via polterware-ui `route_avatar`/`agent_avatar_playing`, e no plugin via plugins/opencode-gui/vendor/bot-avatars + src/avatars.tsx `AgentBot` com a prop `status`) tem expressões genéricas. Mapear cada estado da run para uma expressão/humor próprios e legíveis num relance, no app (sidebar, run page, hover card, listas) e no plugin OpenCode (painel e tela de log):
   - pendente/na fila: olhos meio fechados/esperando; rodando: olhos atentos/pensando (já há as "beats" enquanto trabalha, manter); concluída (COMPLETED): feliz/olhos fechados sorrindo; mesclada (merged): comemorando/orgulhoso (distinto do concluído); falhou (FAILED): triste/assustado; cancelada (CANCELLED): olhos "x" ou cabeça baixa/neutro cansado; timeout (TIMED_OUT): relógio/olhos arregalados cansados; interrompida/órfã (ORPHANED): confuso/perdido; rate limit/fallback: surpreso, se for visível ao status.
   - Reaproveitar o que o bot-avatars já oferece (presets/moods: o plugin usa `EmptyBot mood="sleeping|lost"`; ver types.ts/presets.ts/BotAvatar.tsx); só criar expressões novas onde faltar. Manter o contrato do AGENTS.md: o corpo do bot fica em repouso no sidebar, só o rosto muda; transição suave entre estados (o rosto de uma run que termina muda sem remontar a lista; no sidebar o ponto de "update não visto" continua). Sem custo de layout (a transcrição é uma view em cache). Respeitar reduced-motion se o código já respeita.
   - Testes: mapeamento estado -> expressão como função pura com teste unitário (todos os status do core, inclusive `mergedAt`), teste de snapshot/estado no plugin, e um ui_flows que confirma a expressão na run page de uma run concluída vs falha. Rebuild do bundle do plugin se o plugin mudar. Uma frase no AGENTS.md.

E. Fechar: recompilar o binário `dispatch` / app (o `dispatch setup opencode` instala o bundle EMBUTIDO no binário; enquanto o binário for velho, ele sobrescreve a pasta instalada com a versão antiga e o `dispatch events` nem existe no binário instalado) e reinstalar o plugin (`dispatch setup opencode`). Hoje a pasta instalada em ~/Library/Application Support/Polterware Dispatch/dispatch/opencode-plugin só tem a tela de log antiga, copiada à mão. Depois de recompilar e reinstalar, conferir que os arquivos instalados batem com crates/dispatch-core/src/setup/opencode (rpc.js, server.js, gui-experimental.cjs). Observação: ~/.config/opencode/opencode.json tem uma 2ª entrada de plugin apontando para uma cópia temporária em /private/tmp/claude-501/.../scratchpad/live-copy/dispatch/opencode-plugin, que pode estar carregando no lugar da instalada: avise-me, não remova sem eu dizer. Me peça para reiniciar o OpenCode Desktop e confirmar à vista: tela de log (com log ao vivo numa run ativa), botão Delegate, seletor Agent, e no app Dispatch o dialog Usage, a paginação e os rostos dos bots.

## Estado do repositório quando escrevi isto
- main em 4b1a154 (merges de `dispatch events` e do seletor por cima do merge 7f7eb5f). Havia uma worktree de outra tarefa rodando (9429ccb5505d) e a 95c2b859ff61 pendente (ver Regras). Rode `git status` e `dispatch runs` antes de começar.
- Runs que falharam por falta de saldo do DeepSeek: ac44bdbb88f1 (log ao vivo, trabalho parcial na worktree) e 066fbf214f7c (Usage, sem alterações).
- Brief completo da paginação e do Usage também em /tmp/brief-page.txt e /tmp/brief-usage.txt (se ainda existirem).

Ao terminar, responda com: o que foi mesclado (hashes), testes rodados e resultado, o que não foi verificado à vista, e o que sobrou.

## Chat B: o que eu fiz, o que falta e o que o usuário me pediu

Escrito pelo chat B (orquestrador via OpenCode/Claude), conferido com `git status`, `git log --oneline -15`, `git worktree list`, `dispatch runs` e `dispatch worktrees` em 2026-11-07, por volta das 12:30.

### 1. Pedidos do usuário a este chat (em ordem)
1. "no dispatch gostaria da opcao de desligar ligar os agents": usar "o deepseek do opencode como subagente", "do provider do deepseek", e "lancar pelo dispatch".
2. Mudança de ideia: "nao use deepseek pro, use v4.1 flash". Perguntei qual dos dois, e ele escolheu `deepseek/deepseek-flash` (provider DeepSeek), não `opencode-go/deepseek-v4.1-flash`.
3. "gostaria tambem que desse para apagar e mudar o numero de colunas de cada agent", "mais flexivel a escolha de cada agente", "dispache outro deepseek flash". Esclareceu: são as colunas easy/normal/hard do board de cada role, e "so o general que eh obrigatorio ter todas as colunas".
4. "quando terminar essas entregas": no sidebar agrupado por conversa, ao passar o mouse no título da section, mostrar um tooltip com o nome inteiro da conversa.
5. "animacoes dos bots mais animadas quando ta rodando", "mais pulinhos", "mais animacoes", usando a lista de animações da lib.
6. Interrompeu a espera das runs. Depois perguntou "e ai?", "ainda tem dois agentes rodando", "parecem ta parados", "mas os testes tudo ja passaram sabe? e ta travado", "mas ta rodando desde madrugada, e ainda nao deu certo?".
   - Na pergunta, primeiro escolheu "Esperar mais".
   - Depois disso eu cancelei as duas runs e validei eu mesmo.
7. No merge bloqueado por alterações não commitadas, escolheu "Commitar e depois fazer o merge".
8. Pediu para continuar a partir deste handoff (itens A a E do chat A). Logo em seguida disse "esquece o que falei antes". Não fiz nada desse pedido além de ler.
9. Pedido atual: só registrar este handoff.

### 2. Feito e mesclado
- **Liga/desliga de agentes** (run `f37b7bee0862`, opencode/deepseek-flash, 27 arquivos, +440 −34).
  - **Commits:** a run em `bead0fb`; o merge, que fiz à mão, em `1e19eca`.
  - **O que entrega:** migration `0012_disabled_agents.sql`, RPC `SetAgentEnabled`, `DispatchAgent.enabled`, `dispatch agents enable|disable <name>` e um switch por agente em Settings › Agents. O agente desligado é filtrado em router/routing/classifier, e `--agent` desligado é recusado.
  - **Arquivos principais:** proto, store/dispatch.rs, server/{ops,routes,runs,convert,mod}.rs, coreclient/services.rs, config.rs, router.rs, runner/{routing,execute,mod}.rs, classifier/mod.rs, cli/runs.rs, app/src/{client.rs,views/settings.rs}, testes (e2e_cli, client_e2e, ui_flows, store_sqlite) e AGENTS.md.
  - **Conflito resolvido à mão:** só o AGENTS.md (juntei a frase de Settings > Agents à versão mais nova).
  - **Testes:** `RUST_TEST_THREADS=1 bash tooling/verify.sh` no resultado do merge deu `verify OK` (exit 0). Antes do merge, a pedido do usuário, commitei o trabalho pendente do checkout em `7047eb4` ("Roles palette efforts and card swap, live run log, and runner fixes in progress").
- **Bots mais animados** (run `e9dfdfd0a5c8`, cancelada por mim depois de cerca de 8h30 em ciclo de teste/bench; o trabalho estava pronto na worktree).
  - **`bot-avatar` (main):** `4933307` "Livelier busy bots: frequent hops and body beats from the poke set". Arquivos: src/engine.rs, src/avatar.rs, src/poke.rs, tests/render.rs. Descartei a mudança ruidosa no Cargo.lock (`[[patch.unused]]`).
  - **`ui` (main):** `fd89c5b` "Describe the livelier busy bot beats", só comentários de documentação em crates/polterware-ui/src/bots.rs.
  - **Merge:** os dois foram fast-forward à mão (`git merge --ff-only`).
  - **Testes no bot-avatar:** `cargo fmt --check` ok; `cargo test` 82 + 8 passaram; `cargo clippy --all-targets` com 82/103 warnings, a mesma contagem de antes (pré-existentes).
  - **Não verificado:** `cargo test -p polterware-ui --features bots` não foi rodado, e não vi o resultado à vista no app.
- **Colunas por role** (run `b14bc5b8e8eb`, cancelada por mim depois de cerca de 8h50 sem editar arquivos).
  - **Commits:** commitei a worktree em `a8e0c08`, e `2bbc9ce` ("cargo fmt") formata o app/src/views/roles.rs da main.
  - **Merge:** comecei à mão. Resolvi o AGENTS.md (frase das colunas, `--columns` no uso da CLI e a descrição nova dos bots) e app/src/views/roles.rs (lista de imports da branch, sem `palette_efforts`, que não existe na main). `cargo build -p dispatch-app` ok.
  - **Commit do merge:** `7f7eb5f` (12:11) não fui eu quem fez; provavelmente foi o chat A, a partir do meu merge em andamento.
  - **Testes:** o primeiro `verify.sh` falhou só em `e2e_cli::a_core_started_from_a_background_thread_runs` ("timed out waiting until the core is up", com a máquina carregada). Isolado, ele passou em 19 s. O segundo `verify.sh` (/tmp/verify-b14b.log) parou no meio, sem resultado final.
  - **Não verificado:** o verify completo depois de `7f7eb5f`.

### 3. Feito, mas não commitado, não mesclado ou não verificado
- **Run `9429ccb5505d` (tooltip do sidebar):** status FAILED (DeepSeek sem saldo). Worktree `.dispatch/worktrees/9429ccb5505d375e89b4558cc40dc92e`, branch `dispatch/9429ccb5505d…`, base `2bbc9ce`. Tem alterações sem commit em app/src/models/sidebar.rs e app/src/views/sidebar.rs (+129 −20). Não revisei nem testei.
- **Worktree `e9dfdfd0a5c8` no bot-avatar e no ui:** detached em `4933307` e `fd89c5b`, que já estão na main. No bot-avatar só sobra o `Cargo.lock` modificado (ruído). Pode ser removida, mas não removi.
- **Runs `ac44bdbb88f1` e `95c2b859ff61`:** são do chat A (ver o texto acima). Não mexi nelas.
- **Não verificado à vista no app Dispatch:** o switch de agentes, as colunas por role e as animações dos bots.

### 4. Falta fazer (prioridade, do ponto de vista do chat B)
1. Rodar `RUST_TEST_THREADS=1 bash tooling/verify.sh` na main atual (`4b1a154`), que nunca teve um verify completo depois de `7f7eb5f`.
2. **Tooltip do sidebar:** revisar a worktree `9429ccb5505d`, ou refazer com codex/claude.
   - **Pedido:** no título de cada grupo do sidebar, um tooltip com o nome completo da conversa (nome dado no Rename, senão o primeiro prompt inteiro, limitado a cerca de 500 caracteres); o chevron e as ações mantêm os tooltips deles.
   - **Arquivos:** app/src/models/sidebar.rs (guardar o título sem truncar) e app/src/views/sidebar.rs (perto do `.tooltip(format!("Actions for {}", group.title))`).
   - **Testes:** `cargo test -p dispatch-app` mais um teste de model para o título completo.
   - **Atenção:** conflita com o item C do chat A (paginação no mesmo sidebar.rs).
3. Rodar `cargo test -p polterware-ui --features bots` no `ui` e conferir à vista os bots no app.
4. Remover as worktrees `e9dfdfd0a5c8` (bot-avatar e ui) com `git worktree remove`, depois de confirmar.

### 5. Estado do repositório agora
- **dispatch:**
  - Branch `main`, HEAD `4b1a154`, ahead 15 de origin/main, sem push.
  - `git status`: limpo, sem merge nem rebase em andamento.
  - `git worktree list`: main; `9429ccb5505d` (2bbc9ce, tooltip, sujo); `95c2b859ff61` (7739952, codex, do chat A); `ac44bdbb88f1` (4b1a154, log ao vivo, sujo, do chat A).
  - `dispatch runs`: nenhuma run RUNNING ou PENDING.
  - `dispatch worktrees`: e9dfdfd0a5c8 cancelled/changes, 9429ccb5505d failed/changes, 95c2b859ff61 completed/changes, ac44bdbb88f1 failed/changes.
- **bot-avatar:** main `4933307`, limpo; worktree `e9dfdfd0a5c8` detached e com o Cargo.lock sujo.
- **ui:** main `fd89c5b`, limpo; worktree `e9dfdfd0a5c8` detached.

### 6. Sobreposição com o chat A
- **AGENTS.md:** os dois escreveram no mesmo parágrafo gigante.
  - **Meu lado:** a frase de Settings > Agents, a frase das colunas por role com `--columns`, e a descrição dos bots ("frequent small hops … a rare spin").
  - **Lado do chat A:** Delegate/Agent pill e `dispatch events` no plugin.
  - **Hoje:** os dois lados estão na main. Em conflitos futuros, manter as duas frases.
- **app/src/views/sidebar.rs e app/src/models/sidebar.rs:** meu tooltip (worktree 9429) e a paginação C do chat A. Sugiro fazer primeiro a paginação do chat A e depois reaplicar o tooltip por cima, porque ele é pequeno e localizado.
- **schemas/dispatch/v1/dispatch.proto, app/src/client.rs, server/ops.rs, server/routes.rs, coreclient/services.rs:** meu `SetAgentEnabled` já está na main. O `GetConversationUsage` (item B do chat A) vai tocar os mesmos arquivos. É só acrescentar, não deve haver conflito real.
- **cli/runs.rs e cli/ops.rs:** `dispatch agents enable|disable` (meu) e `--columns` em roles (meu) já estão na main. `dispatch usage --conversation` (chat A) é aditivo.
- **Bots (item D do chat A):** eu mudei o comportamento "busy" (engine.rs, `Calm::Busy`, beats) no bot-avatar `4933307`. O item D muda as expressões por status. Deve partir do bot-avatar atual e manter os beats novos enquanto a run está rodando.
- **runner/routing.rs e router.rs:** meu filtro de agentes desligados e a escolha da coluna mais próxima por role já estão na main.

### 7. Problemas conhecidos e decisões
- **DeepSeek:**
  - A primeira run com `deepseek-v4-pro` (`0591dd29`) foi cancelada sozinha depois de 24 s com "context canceled", causa desconhecida. A `7127c93ec8c5` (também v4-pro) eu cancelei porque o usuário trocou para o flash.
  - Agora a conta DeepSeek está sem saldo ("Insufficient Balance"): não usar deepseek.
- **deepseek-flash entra em ciclo:** faz a mudança certa mas não para, repetindo `cargo test`, `ui_flows` e o bench por mais de 8 h. Decisão: cancelar e validar à mão. Nos próximos briefs, pôr um limite explícito ("rode os checks no máximo 2x; se algo não relacionado falhar, pare e reporte").
- **Testes e2e intermitentes:** falham quando a máquina está carregada (`a_core_started_from_a_background_thread_runs`; o agente de f37 relatou timeouts parecidos). Rodar com `RUST_TEST_THREADS=1` e sem outros `cargo test` em paralelo.
- **Processos de teste antigos:** há processos `dispatch-app`/`dispatch` de testes anteriores em target/debug (PIDs 76009, 79939 e 98892 na época) que ficaram para trás. Ofereci encerrar e o usuário não respondeu, então não mexi.
- **Clippy:** muitos warnings pré-existentes em dispatch e bot-avatar. O `verify.sh` não usa `-D warnings`, então não bloqueiam.
- **Interpretação minha (não pedida explicitamente):** nas colunas por role, uma tarefa numa coluna apagada usa a coluna mais próxima que a role manteve, preferindo a mais difícil; uma coluna vazia usa o General.
