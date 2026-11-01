# ACP на официальном SDK: таблица соответствия

Замерено `crates/ide-agents/examples/acp_probe.rs` на нашей заглушке (`acp_test_agent`), 2026-11-01.
Проба запускает агента так же, как панель — командной строкой, — согласует версию протокола, открывает
сессию, шлёт одну реплику и печатает каждое уведомление и каждый запрос агента как есть.

```
cargo run -p ide-agents --example acp_probe -- "docker exec -i hermes-agent hermes acp" "скажи: проба"
```

## Что уже проверено вживую

- **Согласование версии и возможностей идёт.** Проба просит `ProtocolVersion::V1`, агент отвечает
  `protocolVersion: 1` и перечнем `agentCapabilities` (`loadSession: true` у заглушки). Это тот шаг,
  которого в нашем слое не было.
- **Что мы заявляем о себе** (из перехвата): `clientCapabilities = {fs: {readTextFile: false,
  writeTextFile: false}, terminal: false, auth: {terminal: false}}` — то же решение «файлы агент берёт
  сам, через терминал», но теперь оно часть согласования, а не наша договорённость с собой.
- **Исполнителя SDK не приносит.** Крейт не зависит от tokio; его собственные проверки гоняют соединение
  через `futures::executor::block_on`, и наш поток агента может делать то же — синхронный `Session`
  ложится на SDK без асинхронного движка в проекте.
- **`id` — непрозрачен.** Стандарт разрешает и число, и строку; SDK шлёт UUID. Наш слой и заглушка знали
  только числа: первая проба зависла на `initialize`, потому что заглушка приняла строковый `id` за
  отсутствующий (уведомление) и промолчала. Заглушка исправлена (id переносится как есть).

## Таблица

| Наше (`ide_agents::Event`) | Стандарт, `schema::v1` | Состояние |
|---|---|---|
| `Setup(SessionState)` | `InitializeResponse` (capabilities) + `NewSessionResponse` | есть, но собирается из нескольких мест |
| `Commands(Vec<AgentCommand>)` | `SessionUpdate::AvailableCommandsUpdate` | есть · проверено |
| текст агента | `SessionUpdate::AgentMessageChunk` (ContentChunk) | есть · проверено |
| `Thinking` | `SessionUpdate::AgentThoughtChunk` | есть |
| слова человека (при восстановлении) | `SessionUpdate::UserMessageChunk` | есть |
| `Tool`, `Plan` | `ToolCall`, `ToolCallUpdate`, `Plan` | есть |
| `Usage { used, size }` | `SessionUpdate::UsageUpdate { used, size }` | есть · проверено |
| `Choice { id, request }` (форма с вариантами) | `elicitation/create` → `CreateElicitationRequest` (`mode: form`, `oneOf: [{const,title}]`), ответ `ElicitationAction` | есть · проверено кругом |
| `Permission { id, request }` | `session/request_permission` → `RequestPermissionRequest { tool_call, options: [{optionId, name, kind}] }` | есть в схеме, пробой ещё не гоняли |
| `Sessions(Vec<SessionInfo>)` | `session/list` (`SessionCapabilities.list`) | есть |
| `Finished(reason)` | `PromptResponse.stop_reason` (`EndTurn`, `Cancelled`, …) | есть · проверено |
| `Closed` | закрытие транспорта (EOF) | есть |
| `Failed(String)` | `Error` (JSON-RPC) внутри `schema::v1::Error` | есть |
| `Other { kind }` | любой `SessionUpdate`, которого панель не показывает | есть (в схеме `#[non_exhaustive]`) |
| — (у нас распознаём по тексту) | `SessionUpdate::Notice`, `CompactionUpdate`, `CompactionSummaryChunk` | **есть в стандарте, но Hermes их не шлёт** (`acp_adapter/*` их не собирает) — распознавание по форме остаётся |
| — | `fs/read_text_file`, `fs/write_text_file`, `terminal/*` | агент может спросить; мы заявляем `false`, и SDK на необработанный запрос отвечает отказом |
| — | `_meta` (произвольные поля, напр. `sessionProvenance`) | едет как есть, типизировано как `Meta` |

## Что из этого следует для перевода

1. **Поверхность `Session` сохраняется** (`start_with`, `next_event`, `answer_permission`, `answer_choice`,
   `list_sessions`, `cancel`, `set_model`, `set_mode`, `set_option`, `begin`, `load_into`), и на SDK
   ложится её нутро, а не панель: `crates/andy-ide` не меняется.
2. **Журнал провода сохраняется** — иначе теряется единственное, что отличает «агент молчал» от «панель не
   нарисовала». Схема предлагает для этого свои транспорты (`Lines`, `BytesStreams`, `Channel`), значит
   обёртка вокруг чтения/записи остаётся возможной.
3. **`docs` в `_meta` и `Notice`/`Compaction*`** — то, что стоит начать читать стандартно, но с оговоркой:
   наш агент их не шлёт, поэтому распознавание сводки по тексту убирать нельзя.


## Замер на настоящем Hermes (`hermes-agent 0.22.5`, 2026-11-01)

Одна реплика, полный протокол, 11 строк вывода:

- `initialize` → `protocolVersion: 1`, `agentInfo: hermes-agent 0.22.5`,
  `loadSession: true`, `promptCapabilities: {image: true}`,
  `sessionCapabilities: {list, resume}` — то есть **и список разговоров, и продолжение разговора в
  стандарте есть**, и наш слой их уже использует (`session/list`, `session/load`).
- `authMethods`: `nous` (credentials) и `hermes-setup` (`--setup`) — панель их никак не показывает.
- `available_commands_update` — 14 команд, с `input.hint` у тех, что просят довод.
- **`session_info_update` с заголовком разговора** — и агент переписывает его по ходу («Ответь одним
  словом: проба» → «Проба»). У нас для этого свой склад (`session_names`) и своя кнопка переименования;
  стандарт предлагает и то, и другое на проводе.
- `agent_message_chunk` несёт `messageId` — поля, которого наш разбор не знает.
- `usage_update {used: 25948, size: 1048576}` — настоящие числа контекста.
- `PromptResponse.stop_reason: EndTurn`.

Чего в коротком ходе не было: `_meta.sessionProvenance` (приходит со сжатием), `Notice`,
`CompactionUpdate`. Поэтому распознавание сводки по форме текста остаётся до отдельного замера на сжатии.

## Открытые вопросы

- ~~Проба на настоящем Hermes~~ — сделана (см. выше). Осталась проба **на сжатии**: иначе состав
  `_meta.sessionProvenance` и наличие `CompactionUpdate` у нашего агента неизвестны.
- Реплики заглушки `ask`, `read`, `write`, `slow`, `die` — ими добираются дозволение, файлы, отмена и
  падение.
- Что SDK делает с запросом, для которого у клиента нет обработчика (наш случай для `fs`/`terminal`):
  обещанный отказ или тишина — проверить пробой.
