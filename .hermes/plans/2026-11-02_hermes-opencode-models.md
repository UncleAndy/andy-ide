# Модели OpenCode: что выяснено на живой сессии

**Поставлено Andy 2026-11-02:** «Нужно исправить ошибку в Hermes-agent, которая не позволяет ему нормально использовать модели OpenCode. По факту, модели OpenCode НЕ работают».

**Источник:** https://github.com/NousResearch/hermes-agent/issues/81584 — «OpenCode Go deepseek-v4-flash fails with 400 "Model is unavailable" unless x-opencode-session is sent» (закрыт, PR #101864, релиз v2026.9.7).

## Проверено на живой сессии (2026-11-02, образ `hermes-agent:v0.22.5-andy0.13`)

Ход через ACP-дверь, выбор модели стандартной настройкой `model`, затем минимальный вопрос «Reply exactly with: OK». Авторитет — журнал `/opt/data/logs/agent.log`, а не код:

| Модель | Кто обслужил | Фолбэк |
|---|---|---|
| `opencode-go:glm-5.3` | сама `glm-5.3` | нет |
| `opencode-go:claude-haiku-5-5` | сама `claude-haiku-5-5` | нет |
| `opencode-go:deepseek-v4.1-flash` | **gemini-flash-lite-latest** | да |

Отказ третьей модели — от OpenCode, а не от Hermes:

```
provider=opencode-go base_url=https://opencode.ai/zen/go/v1 model=deepseek-v4.1-flash
HTTP 400: Upstream request failed: This Go model requires Global regions.
          Select Global in your workspace's Privacy settings to use it.
Fallback activated: deepseek-v4.1-flash → gemini-flash-lite-latest (gemini)
```

## Вывод

1. **Ошибки из issue #81584 в нашей сборке нет.** Доказательство не «по коду», а по факту: без заголовка `x-opencode-session` OpenCode отвечает 400 на **любой** Go-модели (так это описано и в самом модуле: `#105841`, «MissingSessionID»). Две модели ответили сами — значит заголовок уходит и маршрут исправен. Сам код тоже на месте: `agent/opencode_affinity.py`, подключён в `chat_completion_helpers.py:1507` и `auxiliary_client.py:6752`.
2. **Отказ `deepseek-v4.1-flash` — свойство аккаунта, а не сборки.** OpenCode отвечает, что этой модели нужен регион **Global**: его выбирают в настройках приватности рабочего пространства OpenCode. Пока он не выставлен, именно эта модель недоступна любому клиенту, не только Hermes.
3. **Наша настоящая вина — молчание.** Hermes ушёл на платный запасной провайдер (gemini) и ничего об этом не сказал: в панели виден нормальный ответ «OK», и человек не знает, что выбранная модель не обслужила запрос. Это ровно тот вредный случай, о котором предупреждает issue («трафик уходит на платные фолбэки без понятной причины»).

## Что делать

1. **Andy:** выставить Global в настройках приватности рабочего пространства OpenCode — или пользоваться теми моделями, что отвечают уже сейчас (`glm-5.3`, `claude-haiku-5-5` проверены).
2. **Нам:** показать фолбэк в панели. Факт уже едет по проводу — в данных сессии есть `fallback_model` (`gateway/session_usage.py::usage_stats`, поле заполняется при `agent._fallback_activated`), но панель его, судя по всему, не показывает. Отдельной небольшой задачей: сказать в панели аген­та, что ответил не выбранный провайдер, и какой именно.

## Чего не делать

Не «лечить» это статическим `x-opencode-session` в конфиге: заголовок исправен, а причина в другом. И не выдавать Hermes за официальный клиент OpenCode.
