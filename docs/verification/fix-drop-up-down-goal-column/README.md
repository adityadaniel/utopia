# Up/Down without a remembered column

Scratch graph and config dir under `target/verify-goal/`, driven with:

```
UTOPIA_SHOT_SCRIPT="wait 1500; type The quick brown fox jumps; keys enter; type Hi; keys enter;
  type The quick brown fox jumps; keys left ×10; shot 1-start; keys up; shot 2; keys up; shot 3"
```

| Shot | What it shows |
|---|---|
| `1-start.png` | The caret after "brown" in the third block. |
| `2-up-to-the-short-block.png` | Up: the caret at the end of "Hi", the nearest place under it. |
| `3-up-keeps-the-short-x.png` | Up again: the caret under the end of "Hi" ("Th\|e"), not back under "brown". No column is remembered between steps. |
