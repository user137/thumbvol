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
- `cargo wix --nocapture` (Windows, потребує `cargo install cargo-wix` + `candle.exe`/`light.exe`
  з WiX Toolset v3 у PATH) — MSI з `wix/main.wxs` у `target/wix/`.
- `cargo deb` (Linux, `cargo install cargo-deb`) — `.deb` з метаданих `[package.metadata.deb]` у
  `Cargo.toml`, у `target/debian/`.

## Неочевидні деталі (gotchas)

- Windows-бекенд перевірено на реальному залізі (MX Master через Logitech Unifying-приймач,
  VID_046D, присутній на машині розробки) — напрямок і чутливість підтверджені наживо. Linux-
  бекенд реально зібрано, протестовано (39/39 тестів справді виконано), пролінтовано (clippy/fmt)
  на живій Ubuntu 24.04 VM — не лише крос-таргет `cargo check` з Windows (той якраз пропустив
  реальний баг у `build.rs`, див. нижче). Помилка "пристрій не знайдено" й запис XDG autostart
  перевірені реальним запуском. Єдине, що досі не перевірено рантаймом — сам шлях
  `open_device`/`build_volume_uinput`/`emit_step` із фізичною мишею (на тестовій VM лише
  синтетична Hyper-V-миша, без `REL_HWHEEL`).
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
- Пакунки (MSI/`.deb`, D10) не керують автостартом — його й далі пише сама програма при першому
  запуску. Перед видаленням пакунка треба `thumbvol --uninstall-autostart`, інакше запис в
  автозапуску переживає видалення й вказує на вже неіснуючий бінарник. Windows MSI —
  `InstallScope='perUser'` (без адмін-прав); компонент з файлом під `LocalAppDataFolder` мусить
  мати `KeyPath` на HKCU-реєстр, не на файл (`ICE38`), і явний `RemoveFolder` для чистого
  видалення (`ICE64`) — реально зібрано й перевірено (`msiexec /i`/`/x`) локально. `.deb` реально
  зібрано й встановлено/видалено на тій самій Ubuntu-VM, що й основний Linux-бекенд;
  `dpkg-shlibdeps` сам визначив `Depends: libc6` — жодних ручних залежностей не знадобилось,
  бо `evdev` не лінкується проти `libudev`. Збірка пакунків — окремий `release.yml`, тригер на
  тег `v*`, не в основному CI (щоб не подовжувати швидкий feedback-loop).

## Три Б

Три ноги й тести — `~/.claude/dev-practices.md` §7. Тут: D1/deferred `SendInput` (користувач),
unquoted-path фікс/`MAX_STEPS_PER_FEED` (софтверна), `input`/`uinput`-групи без root (нижній шар).

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
