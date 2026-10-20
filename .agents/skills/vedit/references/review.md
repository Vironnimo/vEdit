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

## Change reviews

To hand finished work to the user, show every change as one continuous, read-only diff in place of the active project's editor panes. The user scrolls through it and ticks files off as seen:

```powershell
& $VEditCli review --working                 # staged, unstaged and untracked vs HEAD
& $VEditCli review --commit HEAD             # one commit vs its first parent
& $VEditCli review --branch                  # HEAD since it left local main (or master)
& $VEditCli review --branch --base develop 'C:/project'
```

Choose exactly one source. The optional `--workspace` must be an open project with a repository; without it the project containing your current directory is used, else the visible one. With `--background` the user's view stays: a review of another project waits there until the user switches to it; a review of the visible project is prepared and appears with the user's next review command, and the answer says `visible:false`. To point the user at it, follow with `notify ... --review working`. A change review never starts vEdit, does not open tabs, and does not edit, stage or save anything. The response arrives after the comparison loaded and reports `title`, `files`, `added`, `removed` and `seen`; large and binary files appear with a note instead of a patch. `review_closed` or `review_replaced` means the user closed it or another review replaced it before it loaded. Seen marks belong to the user; the CLI only reports their count. `schema change-review` gives the JSON request.

While reading, the user can select diff rows and write a **Comment for agent**. It arrives as an agent mark with a `review` field and the diff excerpt as its text; collect it with `agent-marks take` as described in `agent-marks.md`. A working-tree review does not include unsaved buffers; the user sees how many are left out. When the repository changes after loading, the review only offers **Refresh**; it never reloads by itself.
