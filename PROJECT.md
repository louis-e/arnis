# PROJECT — Arnis Create Rails

## Что это

Форк [Arnis](https://github.com/louis-e/arnis) (Rust, генерация мира Minecraft из данных OSM).
Цель форка: вместо ванильных рельсов (`railway=rail` → `minecraft:rail`) ставить рельсы мода
**Create** — блок `create:track` с корректными block entity данными соединений
(`create:track` BE: `Connections`, `Smoothing`, `BoundLocation`, `BoundDimension`).

Итог: сгенерированный Arnis-мир открывается в Minecraft с установленным Create,
рельсы — это треки Create, по которым ходят вагонетки Create.

## Целевые версии

| Компонент | Версия |
|---|---|
| Minecraft (Java) | 1.21.1 |
| Arnis `DATA_VERSION` | 3955 (= MC 1.21.1, `src/world_editor/java.rs:23`) |
| Create | 6.0.10+mc1.21.1 |

Риск рассинхронизации форматов минимален: версии данных совпадают.

## Текущее состояние форка

- Базовый коммит: `ab80293d` (апстрим, ветка `create-rails`), локальных изменений кода ещё нет.
- Исследование оформлено в `NOTES.md`; план работ — в `ROADMAP.md`.
- `docs/create_track_samples.md` в репозитории **нет** — формат NBT описан по исходникам Create.

## Как Arnis обрабатывает рельсы (upstream, без изменений)

Точка входа — `generate_railways` (`src/element_processing/railways.rs:73`):

- `RAIL_TRACK_TYPES` — типы `railway=*`, для которых генерируется рельс
  (`rail`, `light_rail`, `subway`, `tram`, `narrow_gauge`, `monorail`, `funicular`,
  `miniature`, `preserved`, `disused`).
- Диспетчеризация:
  - `generate_rail_tunnel_shell` — рельс в туннеле;
  - `generate_rail_bridge` — рельс на мосту;
  - `generate_at_grade_rail` — рельс на поверхности (at grade).
- Выбор формы рельса:
  - `determine_rail_with_slope` (`railways.rs:694`) — уклон: если сосед выше,
    возвращается `RAIL_ASCENDING_*` в его сторону; иначе — плоская логика.
  - `determine_rail_direction` (`railways.rs:792`) — направление/поворот по
    тройке (prev, current, next): прямые (`RAIL_NORTH_SOUTH`, `RAIL_EAST_WEST`)
    и повороты (`RAIL_NORTH_EAST` и т.д.).
- `smooth_diagonal_rails` — диагональные сегменты разбиваются на осевые точки
  с промежуточными блоками.
- ID блоков (`src/block_definitions.rs:1206-1215`): `RAIL_NORTH_SOUTH` (116) …
  `RAIL_SOUTH_WEST` (125) — 10 ванильных состояний.

Ключевой вывод: **направление и уклон запечены в ID блока** (разные ID = разные
состояния). Block entity для рельсов Arnis не пишет — всё состояние закодировано
в выборе блока.

## Как Arnis пишет мир (upstream, без изменений)

Модуль `src/world_editor/` (`mod.rs`, `common.rs`, `java.rs`, `bedrock.rs`, `luanti.rs`):

- Формат: Java Anvil (`.mca` region-файлы, палитры блоков, секция `other`),
  зависимости `fastnbt` / `fastanvil`.
- Block entities хранятся в `chunk.other["block_entities"]`.
- Вставка BE: `insert_block_entity` (`mod.rs:1358`) — **уже обобщённая**: принимает
  произвольный NBT-компound (в т.ч. `id` с любым namespace) и кладёт его в список
  `block_entities` чанка. Пример использования — `set_bed_block_entity_absolute`.
- Запись блоков: `set_block` / `set_block_absolute` / `set_block_with_properties_absolute`
  (`mod.rs:2038/2067/2099`). Свойства состояния блока (state properties) уже
  поддерживаются: `BlockWithProperties { block, properties: Option<Arc<Value>> }`
  (`src/block_definitions.rs:59`).
- Палитра пишется как `format!("{}:{}", block.namespace(), block.name())`
  (`common.rs:779`), где `Block::namespace()` **жёстко возвращает `"minecraft"`**
  (`block_definitions.rs:102-104`).
- `Block { id: u16 }` — блок идентифицируется только числовым ID; палитра
  сериализуется в `common.rs:777` и `java.rs:1386`.

Ключевой вывод: путь «блок + свойства состояния» существует
(`set_block_with_properties_absolute`), путь «BE произвольного типа» существует
(`insert_block_entity`). **Не хватает только имени блока мода** — `create:track`
нельзя выразить, пока `namespace()` жёстко `minecraft`.

## Формат Create track (Create 6.0.10+mc1.21.1)

- Блоки: `create:track` (основной) и `create:fake_track` (фейковый блок рядом
  для корректных связей/текстур).
- Block entity: `create:track` — существует **только при `turn=true`**;
  при `turn=false` BE не пишется, всё состояние — в свойствах блока.
- Свойства состояния блока:
  - `shape` — `TrackShape` (76 вариантов: straight/curved/junction/crossing ×
    направления × up/down; полный список — в `NOTES.md` §3);
  - `turn` — bool, true = кривая с сглаживанием (наличие BE);
  - `waterlogged` — bool.
- `TrackShape` несёт оси и направления концов (`getStart`/`getEnd`, `getUp`/`getDown`)
  — по форме определяются соседние блоки и геометрия.
- Структура BE `TrackBlockEntity` (NBT):
  - `Connections` — список `BezierConnection`, по одному на ветвь;
  - `Smoothing` — число, качество сглаживания;
  - `BoundLocation` — локация привязки;
  - `BoundDimension` — измерение привязки.
- `BezierConnection` (на ветвь): `Girder`, `Primary` (bool), `Positions`,
  `Starts`, `Axes`, `Normals`, `Material` (`TrackMaterial`, по умолчанию
  `ANDESITE`), `Smoothing`. Позиции **относительные** — от позиции блока.
- Инвариант `validateConnections`: при загрузке/смене формы Create проверяет
  согласованность `Connections` с формой и соседями; при нарушении —
  пересчёт/очистка.
- `getConnected` (`TrackBlock.java:337-375`): для `turn=false` связи определяются
  только по осям формы (BE не читается); для `turn=true` к связям по осям формы
  добавляются связи из `Connections`.

## Данные, которые нужно считать для каждого сегмента

Для блока трека в `(x, y, z)` с соседями `prev`/`next` (точки и высоты центра линии):

| Поле | Как считается | Зависит от |
|---|---|---|
| `shape` | направление (prev→current→next) + уклон (высоты prev/current/next) | только геометрии |
| `turn` | `false` для straight/junction/crossing и «жёстких» кривых; `true` только для сглаженных кривых | решения генератора |
| `waterlogged` | `false` (или по данным воды) | окружения |
| BE `Connections` | по одному `BezierConnection` на ветвь (curved — 2, junction — 3, crossing — 2): `Girder`, `Primary`, `Positions`, `Starts`, `Axes`, `Normals`, `Material`, `Smoothing`; позиции относительные | формы + соседей |
| BE `Smoothing` | константа генератора (настраивается) | — |
| BE `BoundLocation` / `BoundDimension` | `(x, y, z)` блока / `minecraft:overworld` | координаты |

Критическое упрощение: **пока `turn=false`, block entity не нужны вообще** —
мир полностью описывается состояниями блоков, и `getConnected` работает
только по осям формы. BE появляются только на сглаженных кривых.

## Что нужно изменить в Arnis (diff от upstream)

1. **Имя блока мода** — расширить `Block` (или добавить отдельный путь записи),
   чтобы палитра могла содержать `create:track` / `create:fake_track`:
   `Block::namespace()` перестать возвращать жёсткое `"minecraft"`
   (либо зарезервировать ID и специализировать `try_name()`/`namespace()`).
2. **Константы блоков** — новые ID `CREATE_TRACK`, `CREATE_FAKE_TRACK`
   в `src/block_definitions.rs` (аналог `RAIL_*`).
3. **Генерация** — в `railways.rs` заменить выбор `RAIL_*` на выбор
   `CREATE_TRACK` + свойства состояния (`shape`, `turn`, `waterlogged`)
   через `set_block_with_properties_absolute`.
4. **BE для сглаженных кривых** — новый `set_create_track_block_entity_absolute`
   (аналог `set_bed_block_entity_absolute`), пишущий `Connections`/`Smoothing`/
   `BoundLocation`/`BoundDimension` через существующий `insert_block_entity`.
5. **`fake_track`** — по необходимости (проверить в игре, см. риски).

Формат записи (Anvil, палитры, `block_entities`) менять не нужно.

## Риски и открытые вопросы

- **Имена полей NBT для Vec3** (catnip `VecHelper.writeNBTCompound`) не
  подтверждены: вероятно `{x, y, z}` double — проверить по дампам вайварда
  (поставить трек в игре → сохранить → прочитать NBT).
- **Исходники catnip недоступны** — часть полей NBT (векторы, списки позиций)
  выведена по косвенным признакам.
- **`fake_track`** — семантика размещения (когда и куда ставится) не подтверждена;
  проверить в игре.
- **`validateConnections`** — если `Connections` не согласованы с формой,
  Create пересчитает/очистит данные; нужно решить, писать идеальные данные
  или полагаться на пересчёт (проверить в игре).
- **Namespace** — жёсткий `"minecraft"` требует расширения (п. 1 diff).
- Версии данных совпадают (3955 = MC 1.21.1 = Create 6.0.10+mc1.21.1) —
  рассинхронизация маловероятна.
