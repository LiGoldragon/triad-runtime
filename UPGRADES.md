# Upgrades

## 0.11.0

- Repins the optional `datom-codec` to eabe (0.33.0) and `protos` to 3c7a (0.33.0), from 0.31.0.
- Protos 0.33.0 makes `ReaderBudget` fields private. Code that built `ReaderBudget { remaining: n }` builds it with `ReaderBudget::try_from(n)`. The `datom` test suite does this; the library itself builds no `ReaderBudget`.
- No change to the runtime API. The `datom` feature is unchanged.
