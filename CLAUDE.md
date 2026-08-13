# CLAUDE.md — thumbvol

Легкий крос-платформений (Windows/Linux) демон, що перетворює бічне колесо великого пальця
MX Master на регулятор гучності — заміна важкому Logi Options/Options+ заради єдиної фічі,
яка справді потрібна.

## Архітектура

- `src/core.rs` — платформо-незалежний `WheelAccumulator`: сирі notch-дельти → `VolumeStep`.
  Без I/O, без залежності від ОС.
- `src/config.rs` — TOML-конфіг (`[wheel]` notches_per_step/invert/sensitivity, `[general]`
  autostart, `[linux]` device_path). Валідація один раз, при завантаженні (це і є межа системи).
- `src/platform/windows.rs` — `WH_MOUSE_LL` хук декодує `WM_MOUSEHWHEEL`, глушить подію, інжектить
  `VK_VOLUME_UP/DOWN` через `SendInput`. Автостарт — `HKCU\...\Run`. Трей-іконка (приховане вікно +
  `Shell_NotifyIconW`) з меню: підменю Sensitivity (1–5, радіо-чек), чекбокс Invert, Reload,
  About, Exit. Invert/Sensitivity з меню одразу перебудовують `WheelAccumulator` і зберігають
  `Config` на диск (`Config::save`) — переживає рестарт. `SendInput` виконується не в самому
  `hook_proc` (ризик мовчазного unhook від Windows-таймауту LL-хука на довгій серії викликів), а
  в `wnd_proc` через `WM_VOLUME_STEPS`, яке `hook_proc` лише постить. Іконка — `assets/tray.ico`,
  згенерована `assets/generate-icon.ps1` (перезапустити скрипт після зміни дизайну), вбудована
  через `build.rs` + `embed-resource` + `assets/tray.rc` (resource id `100`, константа
  `TRAY_ICON_RESOURCE_ID` в коді — тримати синхронізованими).
- `src/platform/linux.rs` — читає `REL_HWHEEL` з evdev без ексклюзивного grab (див. DECISIONS.md),
  емітить `KEY_VOLUMEUP/DOWN` через віртуальну uinput-клавіатуру. Автостарт — XDG `.desktop`.
- `src/main.rs` — cfg-диспетчеризація на `thumbvol::platform_current` (аліас на потрібну ОС,
  визначений у `lib.rs`).

## Команди

- `cargo test --lib` — усі юніт-тести (`core`/`config` виконуються на будь-якій ОС; `platform::*`
  компілюється лише на своїй).
- `cargo clippy --all-targets -- -D warnings` — має бути чистим перед будь-яким комітом.
- `cargo build --release` — реліз-білд (`opt-level=z`, `lto`, `strip`, `panic=abort`).

## Неочевидні деталі (gotchas)

- Windows-бекенд перевірено на реальному залізі (MX Master через Logitech Unifying-приймач,
  VID_046D, присутній на машині розробки) — напрямок і чутливість підтверджені наживо. Linux-
  бекенд компілюється й проходить clippy крос-таргетом (`--target x86_64-unknown-linux-gnu`) на
  машині розробки (Windows, без реального Linux), тож API-виклики верифіковані реальним
  компілятором, не лише прочитані з docs.rs. Але жодна рядок цього коду ще не виконувався — доступ
  до `/dev/input`/`/dev/uinput` є лише на справжньому Linux, і жоден юніт-тест його не зачіпає.
  Вважати `platform::linux::run`/`open_device`/`build_volume_uinput` неперевіреними в рантаймі,
  доки хтось не прогнав на реальному Linux-десктопі з реальною мишею.
- Межа кроків в одному тіку `WheelAccumulator::feed` — `MAX_STEPS_PER_FEED = 64`. Знайдено через
  misuse-тест `extreme_delta_*`, який без цієї межі виконувався **125 секунд** (`while`-цикл був
  "доведено обмеженим" лише тезою "жоден пристрій не надішле мільярди", а не з самого рядка).
  Не прибирати межу при правках `feed()`.
- Дефолт `sensitivity = 2`, не `1` — на реальному пристрої підтверджено, що 1 крок гучності ОС
  (~2%) на один клік коліщатка відчувається занадто повільно; 2 (~4%) — комфортне значення. Не
  "спрощувати" назад до 1 без повторного тесту на залізі.
- Крейт `windows` 0.58: `SendInput(pinputs: &[INPUT], cbsize: i32)` приймає слайс (лічильник
  неявний), не сиру C-трійку параметрів; `SetWindowsHookExW(...) -> Result<HHOOK>`.
- `build.rs` мусить гейтити `embed_resource::compile(...)` компайл-тайм `#[cfg(windows)]`, не
  рантайм-перевіркою env-змінної. `[target.'cfg(windows)'.build-dependencies]` резолвиться відносно
  **host**-платформи, що компілює build-скрипт, не таргету — тож на Windows-машині build.rs
  компілюється успішно навіть при `cargo check --target x86_64-unknown-linux-gnu` (крейт є, бо
  host=Windows), а на ubuntu-раннері CI (host=Linux) падає з "cannot find crate embed_resource".
  Знайдено лише реальним CI-білдом на Linux, не крос-таргет `cargo check`/`clippy` з Windows —
  жоден локальний інструмент на цій машині не міг це впіймати.

## Test-first, дисципліна коду

- Спочатку тест, що падає (не компілюється/червоний), потім реалізація.
- Кожна нова фіча покривається трьома категоріями: коректність, rejection (невалідний
  ввід/конструктор), misuse (вироджений/екстремальний ввід, відсутність часткового стану при
  відмові). Приклад: `extreme_delta_saturates_and_is_bounded_instead_of_overflowing` — misuse-тест,
  який реально знайшов DoS-подібний баг (необмежений цикл на зовнішньому вводі з HID-пристрою).
- Керування ресурсами — нативним механізмом мови (`Drop`/володіння), без ручних патернів.
- Межі індексів/лічильників — очевидні з самого рядка (`steps.len() < MAX_STEPS_PER_FEED`), не
  лише з ручного доведення інваріанту через всю функцію.
- `cargo clippy` — це CI-статичний аналізатор проєкту; знахідки виправляються в тому ж проході,
  не залишаються "бо тести пройшли".
- Валідація — лише на межі системи: `Config::parse`/`WheelAccumulator::new` (зовнішній
  ввід/конфіг), не між внутрішніми функціями, що й так гарантують інваріант одна одній.

## Git

- Нові коміти лише за проханням користувача.
- Перед комітом — `git status` після `git add`, не довіряти назві файлу щодо секретів.
