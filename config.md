# Taillight Configuration Guide

`taillight` allows you to save and restore your customized tabs, pane configurations, layout, and highlight queries.

---

## Configuration File Lookup Order

On startup, `taillight` attempts to load configuration files from the following paths in order, using the first one that exists:

1. **Local project directory**: `./taillight_config.json` (resolved against the shell's active working directory).
2. **User home directory**: `~/.taillight_config.json`
3. **XDG config directory**: `~/.config/taillight/config.json`

> [!NOTE]
> Unlike other tools, `taillight` does **not** write to or modify your config files automatically when quitting. To save your current view layout or custom tabs, you must explicitly export it by pressing **`e`**.

---

## Configuration JSON Schema

Here is an example `taillight_config.json`:

```json
{
  "layout": "SplitVertical",
  "active_pane_idx": 0,
  "tabs": [
    {
      "name": "All",
      "severity_filter": null,
      "regex_filter": null
    },
    {
      "name": "Error",
      "severity_filter": "Error",
      "regex_filter": null
    },
    {
      "name": "API Requests",
      "severity_filter": "Info",
      "regex_filter": "POST /api/v1"
    }
  ],
  "panes": [
    {
      "tab_index": 0,
      "highlight_query": "DB_CONNECTION_FAIL"
    },
    {
      "tab_index": 2,
      "highlight_query": null
    },
    {
      "tab_index": 0,
      "highlight_query": null
    },
    {
      "tab_index": 0,
      "highlight_query": null
    }
  ]
}
```

---

## Fields & Options Reference

### `layout`
Sets the TUI layout mode.
* **Type**: String
* **Options**: `"Single"`, `"SplitVertical"`, `"SplitHorizontal"`, `"Split2x2"`

### `active_pane_idx`
Index of the pane that gets focus on startup.
* **Type**: Integer (0-based)
* **Limits**: Must be `<` than the number of panes in the selected layout (e.g. `0` for Single, `0`-`1` for Splits, `0`-`3` for Split2x2).

### `show_timestamps`
Determines whether parsed timestamps are shown on startup. If not specified, defaults to `true`.
* **Type**: Boolean
* **Options**: `true`, `false`

### `ctrl_c_behavior`
Determines what happens when Ctrl-C is pressed while running in piped mode. If not specified, defaults to `"KillAll"`.
* **Type**: String
* **Options**: `"KillAll"` (stops pipeline and exits), `"KillWriter"` (stops pipeline but keeps taillight running)

### `word_wrap`
Determines whether long log lines are word-wrapped. If not specified, defaults to `false`.
* **Type**: Boolean
* **Options**: `true`, `false`

### `filters`
A list of filter configurations. On startup, background tasks concurrently scan log indices matching these filters.
* **Type**: Array of Filter Objects
* **Properties**:
  * **`name`** (String, Required): Display name of the filter.
  * **`severity_filter`** (String or null, Optional): Restricts log lines to matching levels. Checked **case-insensitively**. Options:
    * `"Trace"`
    * `"Debug"`
    * `"Info"`
    * `"Warn"`
    * `"Error"` (Matches both `"Error"` and `"Fatal"`)
    * `"Fatal"`
    * `null` (No severity limit)
  * **`regex_filter`** (String or null, Optional): Regex string pattern. Checked case-sensitively by default. Prepend **`(?i)`** for case-insensitive matching.

### `panes`
Specifies which filter index is opened and what text highlights are applied to each pane.
* **Type**: Array of Pane Objects (Exactly **4 entries** must be provided to support all layout transitions).
* **Pane Properties**:
  * **`filter_index`** (Integer, Required): The index of the filter mapped to this pane (refers to the `filters` array index).
  * **`highlight_query`** (String or null, Optional): Regex pattern to highlight and jump (`n`/`N`) through logs. Checked case-sensitively by default; prepend **`(?i)`** for case-insensitive matching.
