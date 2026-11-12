# Integration Summary: ledface_layout + KiCad Generator

This subproject provides a compact, `no_std`-friendly layout format and parser
for LED watchfaces. It is intended to be used by firmware at runtime and by
tooling at build time.

## What this subproject provides

- JSON schema for a watchface:
  - Physical LEDs: index, name, and X/Y position in millimeters.
  - Logical entities: named groups with arbitrary LED index mapping.
- Runtime model:
  - `Layout`, `Led`, `Entity` data structures.
  - `Entity::set_degrees(u16)` helper to fill a logical group by angle.
- Parsing:
  - JSON parsing via `serde_json_core` (no `std` required).
  - Binary parsing via `postcard` (for firmware-friendly blobs).

## How it fits into a KiCad generator

Goal: one tool that produces:
1) KiCad schematic (symbols + nets)
2) PCB layout metadata (placement, routing constraints)
3) Firmware layout code/data (this crate’s JSON + optional compiled bytes)

Recommended pipeline:

1) KiCad generator emits a canonical watchface model:
   - LED objects with positions, electrical nets, and component refs.
   - Logical entities (rings, bars, custom groups) with ordered LED lists.
2) From that model, emit:
   - KiCad schematic/library output.
   - PCB metadata (e.g., placement CSV, net labels, keepout hints).
   - `ledface_layout` JSON for firmware/tooling.
3) Build step compiles JSON to a binary blob:
   - Firmware embeds the blob and uses `Layout::from_bytes`.
4) Optional codegen:
   - Emit Rust constants or lookup helpers for entity names or indices.

## Schema alignment suggestions

To keep the generator and firmware aligned:

- Make LED indices stable and unique across revisions.
- Use entity names as stable identifiers (avoid renaming without version bump).
- Store physical coordinates in millimeters (matches PCB tools).
- Add optional entity metadata into `params` when you need layout-specific logic.

## Suggested future extensions

- Add a `watchface.id` field for runtime selection.
- Add `rotation` or `origin` metadata to simplify coordinate transforms.
- Add `pcb_ref` for linking LEDs back to schematic references.
- Add optional `polarity` or `driver_channel` fields if needed for routing.

## Merge strategy with the KiCad tool

Short term:
- Generator emits JSON in the current schema.
- Firmware uses the build script to embed a compiled blob.

Medium term:
- Generator emits both JSON and Rust helper code.
- CI validates JSON schema and binary roundtrip.

Long term:
- Single CLI tool that:
  - Reads a high-level watchface description.
  - Produces KiCad outputs + firmware assets in one run.
