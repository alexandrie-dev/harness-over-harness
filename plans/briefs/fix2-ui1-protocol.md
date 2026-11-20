# ui1: second, short round on the approval path

The closure check (read-only, it attacked the path from the main window) closed items 1–4 and 6–8 and found no
way through. Close the remaining gaps; each with a test that fails before the fix.

1. **Tests with a real key.** Every test uses the all-zero key, under which a bug that ignores the key would pass.
   Add Node-computed vectors (command in the comment, as before) with a random non-zero key, including text with
   escapes, an astral character, a float and a nested array in the action; a wrong-key negative test.
2. **Weak keys.** Reject a key of the wrong length and the all-zero key at `initialize`.
3. **The key in Debug.** `HohInitialize` derives `Debug` over the plaintext key; redact it (and any other type
   carrying the key or a MAC secret).
4. **One list.** `PRIVILEGED_METHODS` and `requires_host_approval` must not be two lists that can drift: derive one
   from the other, or a test that they agree.
5. **More privileged methods.** Add to the gated set (and to `docs/window.md` §5 item 6's list in your notes — I
   update the design): `session/working-directory/set` (where the agent may write), `hoh/settings/set` touching
   `model.baseUrl` or any security field, `hoh/model/probe` with a `baseUrl` (outbound connection to an arbitrary
   address). Check whether `hoh/employee/update` with `taskIds` widens rights; if it does, gate it.
6. **What the user reads.** Limits on `humanText` (length) and `risk` (enum) and `expiresAt` (range); reject or
   escape bidi control characters and other invisible formatting in `humanText` so the dialog cannot be spoofed.
7. **Canonical sort.** The action hash sorts keys by UTF-8 bytes; a JavaScript verifier sorts by UTF-16 code units.
   Pick one and state it in PORT_NOTES as normative; a test with keys where the two orders differ.
8. Fixtures for `HostApprovalRequest` and `HostApprovalDecision` so the byte-for-byte test covers them; a test that
   swapping two fields in a type fails.

Report: per item what changed, result lines. At most 250 words.

## Добавлено ведущим 19.11 ~00:50 (веха «окно-чат», вид A; экраны написал s14)

После пунктов выше — недостающие методы, которые экраны уже вызывают через адаптеры (`ui/src/screens/firstrun/engine.ts`,
`ui/src/screens/approval/bridge.ts`; там же их ожидаемая форма). Добавь в `crates/hoh-protocol` (типы, реестр, права,
фикстуры, `scripts/gen-protocol-ts.mjs` → `ui/src/protocol.ts`) и в заглушку `crates/hoh-window-stub`:
1. Сохранение API-ключа модели движком через hoh-credentials (ключ в окне не хранится, в журнал и Debug не попадает);
   привилегированный; связь с `hoh/model/probe` (проверка с сохранённым ключом).
2. Сохранение правил первого запуска: режим интернета, папка для работы — привилегированные (вид действия для MAC).
3. Команды ядра Tauri, которые ждут экраны: `engine_request`, `approval_current`, `approval_decide`, выбор папки —
   опиши их в `docs/window.md` §4 (только описание; реализацию моста делает пакет 4, не ты), чтобы имена и формы
   совпадали с адаптерами s14; если форма адаптера противоречит протоколу — правь адаптер (только эти два файла) и
   напиши почему.
Тесты: круг-в-круг каждой фикстуры, права, заглушка отвечает на новые методы.
