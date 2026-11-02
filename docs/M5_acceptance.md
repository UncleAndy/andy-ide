# Приёмка этапа М5

Отчёт о том, что из критериев приёмки М5 (`docs/TZ_andy_ide_v0.1.md`, раздел 8) закрыто, чем именно проверено и с какими числами. Составлен 2026-11-02 по состоянию ветки `main`.

Формулировки §3.5 («Агент видит проект», «правка файла»), §4.5, §6 и §8 приведены к тому, как среда работает: агент получает корень проекта как `cwd` беседы, файлы читает и правит сам своим терминалом, редактор их не читает и не пишет по его просьбе и предварительного подтверждения правки не требует. Разбор изменений остаётся за человеком — вручную, средствами панели `git`.

## Итог

| # | Критерий ТЗ | Как проверено | Результат |
|---|---|---|---|
| 1 | Диалог с Hermes в панели | живая беседа через `hermes acp` (контейнер, `hermes-agent:v0.22.5-andy0.13`); `alt_0_shows_the_panel_with_the_agents_from_the_file`, `enter_in_the_agent_field_stays_out_of_the_editor`, `a_row_the_agent_wrote_itself_is_an_aside_and_not_the_words_of_the_person`; поток ответа — `the_answer_streams_in_pieces` | выполнено: ответ приходит по мере генерации, слова человека и агента различимы |
| 2 | Агент получает корень проекта | `the_agent_is_told_which_directory_the_project_is_in`; `the_client_asks_nothing_of_itself_and_says_files_go_through_the_terminal` | выполнено: `cwd` уходит в `session/new`, рядом `_meta.fileAccess` с пояснением, что файлы агент читает своим терминалом там же |
| 3 | Файлы агент читает и правит сам | те же две проверки; живая работа агента в каталоге проекта | выполнено; редактор запросов `fs/*` не обслуживает и правок за агента не применяет |
| 4 | Разрешение на команду запрашивается явно | `a_permission_that_is_asked_reaches_the_panel_and_waits_for_the_answer`, `a_permission_that_is_allowed_is_taken`, `a_permission_that_is_refused_is_refused_in_the_agents_own_option`; панель — `the_agent_can_ask_the_person_to_choose_a_variant`, `a_question_of_the_agent_reaches_the_desk_when_the_panel_is_away` | выполнено: ход останавливается до ответа человека, отказ возвращается словами протокола |
| 5 | Второй агент — правкой `agents.toml`, без изменения кода | панель берёт список агентов из файла: `alt_0_shows_the_panel_with_the_agents_from_the_file`; реестр читается из `agents.toml`, копия проекта важнее пользовательской; второй агент — заглушка `crates/ide-agents/src/bin/acp_test_agent.rs` — подключён в проект **записью в файл** и на ней проверялись карточки разрешений | выполнено: второй агент появился без правок кода; в наборах клиента заглушка запускается напрямую (`AgentSpec` в тесте), потому что проверять надо клиент, а не чтение файла |
| 6 | История сессий переживает перезапуск | `a_kept_conversation_is_picked_up_by_its_id`, `a_start_says_which_step_it_is_at`, `a_conversation_the_agent_forgot_is_opened_anew`; панель — `the_project_keeps_the_agent_that_was_on_duty_and_opens_it_again` | выполнено: панель возвращается к той же беседе и не заявляет «resumed» вслепую |
| 7 | Падение агента не влияет на остальное | `an_agent_that_dies_mid_turn_is_reported_and_not_waited_for`, `an_agent_that_says_nothing_is_given_up_on`, `a_turn_can_be_stopped_without_ending_the_session` | выполнено: смерть процесса замечается сторожем, ответа ждут до срока, остановка хода не кончает беседу |
| 8 | Ответ агента не блокирует интерфейс (общий критерий §8) | два потока на агента; `the_setup_row_makes_room_for_the_conversation_while_the_agent_runs` | выполнено: окно перерисовывается во время ответа |

## Числа

Прогон целиком на коммите `aa4a712` (далее только документы): **770 тестов, 0 провалов**, `cargo fmt --check` и `cargo clippy --workspace --all-targets -- -D warnings` без замечаний.

| Набор | Тестов |
|---|---|
| `cargo test -p andy-ide` (окно и панели) | 496 |
| `crates/ide-agents/tests/conversation.rs` (приёмка клиента) | 18 |
| `crates/ide-agents/tests/sdk_session.rs` (поведение сессии) | 17 |
| Остальные наборы (крейты, интеграционные) | 239 |
| Всего | **770** |

Семь проверок, обслуживавших снятую машинерию файлового сервиса (4 в `sdk_session.rs`, 3 в `conversation.rs`), убраны вместе с ней — отсюда 770 против прежних 776.

## Что осталось за этапом

Ничего из §8. Ближайшее — М2 (список реализаций трейта, иерархия входящих вызовов, автодополнение) и М6 (доводка). Отчёт по М4 не составлен по решению от 2026-11-02: там предстоят переделки.
