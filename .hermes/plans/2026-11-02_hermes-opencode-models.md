# Модели OpenCode: проверка на живой сессии

**Поставлено Andy 2026-11-02:** «Нужно исправить ошибку в Hermes-agent, которая не позволяет ему нормально использовать модели OpenCode».

**Источник:** https://github.com/NousResearch/hermes-agent/issues/81584 — «[Bug]: OpenCode Go deepseek-v4-flash fails with 400 "Model is unavailable" unless x-opencode-session is sent». Открыт 08.08.2026, **закрыт**; исправление влито через PR #101864 и вошло в релиз **v2026.9.7**.

## Что об этом известно точно

Суть ошибки: Hermes не отправлял заголовок `x-opencode-session`, и первый же запрос к OpenCode Go/Zen для `deepseek-v4-flash` падал с HTTP 400 «Model is unavailable». Матрица контролей из issue (тот же ключ, модель, тело запроса):

| Заголовки | Ответ |
|---|---|
| без session-заголовка | 400 `Model is unavailable` |
| только `x-opencode-client: pi` | 400 |
| только `x-opencode-session: <uuid>` | 200, полноценный ответ |
| оба | 200 |

Предложенное и принятое исправление — отправлять стабильный непрозрачный `x-opencode-session` для одной беседы на всех маршрутах OpenCode (chat_completions, anthropic_messages, responses), следя за ротацией и сжатием сессии, **не** выдавая себя за официальный CLI (`User-Agent: opencode-cli`, `x-opencode-client: cli` — запрещены).

## Состояние нашей сборки (на 2026-11-02)

Исправление **уже здесь**:

- `agent/opencode_affinity.py` — `opencode_session_headers()`, `custom_provider_session_affinity_headers()`, `merge_session_affinity_headers()`;
- подключено в `agent/chat_completion_helpers.py:1507` и `agent/auxiliary_client.py:6752`;
- проверки: `tests/agent/test_opencode_session_affinity.py` («rides on every OpenCode request, on every transport») и `tests/agent/test_session_affinity_header.py`.

Наша ветка `andy-ide-fixes` построена на канареечном коммите от 28.10.2026, то есть **новее** релиза с исправлением.

## Что делать

1. **Проверить на живой сессии**: настроить провайдера OpenCode Go/Zen с ключом Andy и задать простой вопрос на модели `deepseek-v4-flash` (и на `mimo-v2.5` для контроля — она работала и без заголовка). Ожидаемо: ответ приходит, без 400.
2. **Убедиться, что заголовок действительно уходит** — по журналу провода или отдельной пробой, а не «по коду»: код может быть на месте, а путь запроса — нет (ровно так было с балансом кредитов).
3. **Если откажет** — разбираться дальше: какой маршрут выбрала сессия (`chat_completions` против `anthropic_messages`/`responses`), доходит ли заголовок до клиента в ACP-двери, и не перетирает ли его другой слой.

**Чего не делать:** не выдавать Hermes за официальный клиент OpenCode и не добавлять статические значения заголовка в конфиг как «лечение» — в issue прямо сказано, что это ломает замысел prompt-кэша и маскирует причину.

## Примечание

Если Andy имеет в виду **не** этот случай, а другой сбой с моделями OpenCode (иная ошибка, другой маршрут), — запись нужно поправить: в issue описано ровно одно поведение — 400 «Model is unavailable» без session-заголовка.
