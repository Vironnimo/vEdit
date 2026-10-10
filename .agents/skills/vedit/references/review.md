# Review manifests

This reference covers multi-file reviews that replace the current review annotations. Replace example paths, IDs and positions with values from the task and current buffers. Save a UTF-8 JSON file and run:

```powershell
& $VEditCli review 'C:/project/review.json'
```

Use actual absolute file paths and line positions from the current artifact. One manifest can open many tabs and attach notes to each important range:

```json
{
  "title": "File switching review",
  "note": "Inspect the cache policy and the default settings.",
  "files": [
    {
      "path": "C:/project/plan.md",
      "view": "preview",
      "annotations": []
    },
    {
      "path": "C:/project/cache.rs",
      "view": "source",
      "location": { "line": 6, "column": 5 },
      "annotations": [
        {
          "id": "cache-constructor",
          "start": { "line": 6, "column": 5 },
          "end": { "line": 8, "column": 5 },
          "note": "Check whether the cache budget is appropriate.",
          "severity": "info"
        }
      ]
    },
    {
      "path": "C:/project/settings.toml",
      "location": { "line": 5 },
      "annotations": [
        {
          "id": "cache-default",
          "start": { "line": 5 },
          "note": "Confirm the default matches the plan.",
          "severity": "warning"
        }
      ]
    }
  ]
}
```

`title` names the review; `note` adds optional overall context. Each file specifies `path`, optional `location`, optional `view` (`source` by default), and its `annotations`.

Without `location`, a marked file reveals its first annotation when opened. Supply `location` to reveal another position. Opening or reviewing an already-open clean buffer refreshes it asynchronously from disk; unsaved text is preserved. Verify marks after the refreshed contents finish loading.

Supply `start` and a helpful `note` for each annotation. `id` is optional; supplied IDs must be unique within each file. `severity` is `info`, `warning`, or `critical`, with `info` as the default. An omitted `end` marks the entire start line. With `end`, both endpoints are inclusive. Lines and columns start at 1; columns count Unicode scalar values, including spaces and tabs, rather than UTF-8 bytes or screen cells. An omitted column defaults to 1.

Review submission replaces the previous review annotations. Unknown fields and invalid manifests are rejected before that replacement. Filesystem or later load failures can still leave a partially applied review: inspect each item and the current editor state. For successful items, retain their returned tab IDs. Verify pending items after loading; correct failed paths or ranges before submitting a complete replacement when needed.

`schema review` gives the current review request examples and defaults. Use `request JSON_FILE` for structured actions when ordinary commands do not cover the task.
