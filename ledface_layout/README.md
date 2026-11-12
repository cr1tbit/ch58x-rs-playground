# ledface_layout

Compact, `no_std` layout format + parser for LED watchfaces.

## Purpose

This crate defines a JSON schema for mapping logical entities (rings, bars, groups)
to physical LED indices, plus parsing helpers that work without `std`. It is designed
to be shared across firmware projects and tooling.

## What it provides

- Data model for watchface layouts (`Layout`, `Led`, `Entity`).
- Entity helpers like `set_degrees(u16)` to fill a range proportionally.
- JSON parsing via `serde_json_core`.
- Binary layout parsing via `postcard` (used by firmware to avoid runtime JSON parsing).

## JSON schema (current)

Top-level fields:

- `watchface`: optional metadata.
- `leds`: array of physical LEDs.
- `entities`: logical LED groupings.

LED entry:

- `index` (u16)
- `name` (string)
- `x_mm` / `y_mm` (float)

Entity entry:

- `name` (string)
- `type` (group | bar | ring | matrix)
- `map` (array of LED indices in logical order)
- `params` (optional type-specific metadata)

Example snippet:

```json
{
  "watchface": { "name": "Test Layout", "version": 1 },
  "leds": [{ "index": 0, "name": "LED_0", "x_mm": 0.0, "y_mm": 0.0 }],
  "entities": [{ "name": "Wave", "type": "ring", "map": [0] }]
}
```

## Usage in firmware

Typical flow:

1) Include JSON at build time and compile to postcard bytes.
2) Load the postcard bytes at runtime to avoid JSON parsing.
3) Use `Entity::set_degrees` to update a per-LED frame buffer.

The firmware in `papajbadge-rs` currently includes a build script that reads
`test/test.json`, serializes it with `postcard`, and embeds the blob.

## Tests

- `cargo test -p ledface_layout` runs parsing and JSON↔binary roundtrip tests.

## Notes

This crate is intentionally minimal. Schema changes should keep JSON stable or be
versioned via `watchface.version`.
