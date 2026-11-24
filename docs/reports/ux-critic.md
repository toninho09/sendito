# UX critic — final report

The UX critic is a fresh, isolated agent that sees only the UX bundle built by
`scripts/critic-bundles.sh`: `BRIEF.txt`, `README.md`, `KEYMAP.md`, the screenshots
(120×40 and 200×60, dusk and daylight) and every frame of the 12 scripted
recordings (120×40). It never sees code or earlier reports. Each iteration used a
new agent.

`BRIEF.txt` lists the intentional features (the critic judges how well they work,
never whether they exist) and the settled design decisions (the critic may report a
flaw in how one is carried out, not ask for the opposite choice).

## Iteration history

| Iteration | Blocker | Major | Minor | Nit | Outcome |
|---|---|---|---|---|---|
| 1 | 0 | 3 | 19 | 8 | all fixed |
| 2 | 0 | 5 | 12 | 9 | all fixed |
| 3 | 0 | 2 | 15 | 4 | all fixed |
| 4 | 0 | 2 | 16 | 6 | all fixed |
| 5 | 0 | 3 | 17 | 6 | all fixed |
| 6 | 0 | 4 | 10 | 4 | all fixed |
| 7 | 0 | 2 | 9 | 6 | all fixed |
| 8 | 0 | 2 | 10 | 9 | all fixed |
| 9 | 0 | 4 | 8 | 3 | all fixed |
| 10 | 0 | 1 | 9 | 3 | all fixed |
| 11 | 0 | 3 | 9 | 7 | all fixed |
| 12 | 0 | 4 | 11 | 4 | all fixed |
| 13 | 0 | 2 | 13 | 4 | triaged: 15 fixed, 4 declined |
| 14 | 0 | 2 | 6 | 5 | triaged: 8 fixed, 5 declined |
| 15 | — | — | — | — | stopped; the grid minimum changed from 80×24 to 120×40, so the bundle was rebuilt |
| 16 | 0 | 2 | 6 | 8 | both majors came from stale 80×24 screenshots left in the bundle (removed); the rest triaged |
| 17 | 0 | 2 | 6 | 2 | triaged: 1 major fixed, 1 declined; minors fixed or declined |
| 18 | 0 | 1 | 9 | 6 | triaged: the major was a feature that already existed; new recording frames added; most minors fixed or declined |

**The critic never reached "nothing above minor".** A fresh critic found one to
four new majors every round. Many of them asked for the opposite of what an earlier
round had asked for (Enter in dialogs, palette pre-selection, tab names, history
rows, status-line separators, jargon versus precision). After iteration 12 the
project adopted the triage rule below, and every later finding was either fixed or
declined with a reason. The count in the last round is one major, and that major
was a state the application already showed (see "Declined findings", iteration 18)
but that no recording displayed; the recording was added.

No iteration ever reported a blocker.

## Triage rule

For each finding: fix it when it is a real defect; decline it when it contradicts
an earlier round's settled decision or does not hold up against the product. Every
decline is recorded below with its reason. The settled decisions are in the bundle
`BRIEF.txt`:

1. In dialogs Enter always confirms (Ctrl+S too); on a choice row Left/Right change
   the value and Space opens the list; the button names the action, e.g. "Save (Enter)".
2. An unsaved new tab keeps a stable name ("untitled", "untitled 2").
3. History rows show method, status, name, the URL path, time, and the environment
   when the sidebar is wide enough.
4. The command palette highlights its first row, "Show all keys".
5. `n` in the tree creates the file at once; Ctrl+N opens a scratch tab with no file.
6. The hint row lists keys by priority; "F1 all keys" is always shown.
7. The gRPC message log is the "Log" tab for every call kind.
8. The status line's "last request:" describes the last request sent; other actions
   report in the message area on the right.
9. Values derived from a file are italic and marked "(auto)".
10. Transient messages appear in the message area, apart from the "|"-separated fields.

## Redesign — decisions revised

The 0.2.0 visual redesign was allowed to change the composition and to revisit the
settled decisions above. It was validated with tests, snapshots and a before/after
review of the screenshots, without new critic rounds. What changed:

| Before | Now |
|---|---|
| Focused pane with a `THICK` border | `ROUNDED` border in the accent color; `THICK` only in mono and `contrast` (`Theme.heavy_focus`) |
| Pane tabs on their own row, separated by "│" | Tabs in the pane's top border |
| URL inside the Request pane | Its own strip above both panes |
| A dialog clears the screen (iteration 1 finding) | Dimmed backdrop and a shadow; mono still clears |
| Status "env │ last request: … │ ms │ B" (decisions 8 and 10) | Segments without "│"; the environment only in the top bar; the last request only while the Response pane is hidden; messages in a chip with an icon |
| Start screen = a list of keys | Wordmark, collection summary, Start and Recent |
| `[x]` | `[✓]` |
| Hint row and status line on two rows | One footer row: status segments on the left, key hints on the right with short key names (`^S`, `⏎`); a note replaces the hints for 10 s; `[ui] hints` = `full` / `short` / `off` (decision 6 holds: "F1 all keys" always shows) |
| `Ctrl+W` pending: a fixed hint line | A which-key box at the bottom right, built from the keymap, grouped; a click runs the entry |
| Help in one column | Two columns on screens 180 columns wide or more |
| Kept | Decisions 1–5, 6, 7 and 9 |

## Fixed in the last triaged rounds (13–18)

* Dialogs: action-named confirm buttons ("Create (Enter)", "Save (s)"), the confirm
  hint names the focused button, cancelling the unsaved-changes or quit dialog sets a
  status, the delete cancel says nothing was deleted.
* Feedback: a created request says "created … (an empty file)", a copy says "sent to
  the terminal clipboard" at once, a failed request shows the reason ("connection
  refused · details in the response pane"), the Postman import reports "N entries …"
  and the status says what was imported, the Results tab shows "2/3 ✗".
* Layout: the multipart Filename column is wider so "(auto)" fits; header "Value /
  path"; the tab bars have no dangling separator; the Postman import note wraps on
  two lines; the timing table has short skip reasons and a bar legend.
* Environment editor: "● unsaved" in the title, a legend that mentions Ctrl+S and the
  Esc prompt, the delete-row key kept in the hint row at narrow widths.
* Contrast: the daylight border and dim colors were darkened (border `7d8494`, dim
  `3e434e`); the running badge uses the accent colour so it differs from warnings.
* Wording: "F1 all keys" everywhere, "method none ▾" on an unset gRPC call, "release to
  move X into Y/" while dragging, the undefined-variables dialog points to Ctrl+W V.
* Mouse drag in the tree reports "nothing moves" outside a folder.
* The test grid minimum is 120×40 (it was 80×24); the stale 80×24 snapshots and
  screenshots were removed.

## Declined findings

Each entry gives the iteration, the finding, and why it was declined.

**Iteration 13**
* "Saved" and "unsaved" contradict after creating a request: contradicts the settled
  `n` / Ctrl+N decision; the wording was fixed instead ("created … (an empty file)").
* "Say copied": earlier rounds asked for "sent to the terminal clipboard" because a
  terminal may block the copy; the status keeps the honest wording.
* Tab markers: contradicts the stable-name decision (the URL shows in the bar).
* Hidden "m move" hint: the hint row is priority-ordered and "F1 all keys" lists it.
* "OK (0)": the standard gRPC status name plus code, consistent with "NOT_FOUND (5)".

**Iteration 14**
* "Save (s)" versus "Save (Enter)": question dialogs label the letter keys; only
  input dialogs say "(Enter)" (settled decision 1).
* "set" marker low contrast in daylight: it uses the dim colour, measured at about 9:1.
* Active sidebar list not marked when unfocused: the label is bold accent against a
  dim neighbour.
* Environment missing on history rows: shown when the sidebar is 40+ columns wide; the
  narrow sidebar keeps room for the path and the time. The BRIEF was corrected.
* "Cancel order: description not imported" under Skipped: Sendito has no description
  field, so the item is a loss, which is what the section lists.

**Iteration 16**
* Enter inserts a newline in the Import dialog: the field takes multi-line curl
  commands; the note and button say F5 / Ctrl+S imports.
* The method picker opens with "POST" typed: the picker is opened by typing a letter
  and the recording typed the whole word.
* Daylight contrast: measured at 4.5:1 or better for every foreground colour.
* "request finished" repeats the last-request fields: an earlier round asked for it.
* The cursor on the first letter of "base_url": the terminal's own cursor.
* The bare "+" tab button: the welcome list shows Ctrl+N; the glyph is a standard one.
* The underline on the active sub-tab is faint: it is bold, accent-coloured and
  underlined.
* Findings about 80×24 screenshots: the files were stale leftovers, not shipped.

**Iteration 17**
* "Enter choose method" while the message editor is focused: focus was on the method
  control (it is highlighted); the hint describes the focused widget.
* Theme names not visible: the explanation line lists them when Theme is selected.
* Placeholder contrast on dusk: measured at 4.5:1 or better.
* Separator before transient messages: settled decision 8 / 10.

**Iteration 18**
* "Environment editor has no unsaved state": it shows "● unsaved" in the title once
  there are edits, and Esc asks Save / Discard / Cancel. No recording showed it, so
  two frames were added (04-switch-env/05–06) and the legend now says so.
* Status messages run into the last-request fields: settled decision 8 / 10; added
  to the BRIEF.
* gRPC message template has no visible key: it is `:template` in the palette and in
  KEYMAP.md; the hint row stays short.
* Space flips Kind in the multipart table while Body type opens a list: a table choice
  cell cycles with Space and the hint says "Space change kind"; the Body type is a
  pane field with a picker.
* Body tab, two meanings of Enter: "Enter or Space to change" (Body type) and
  "Enter/↓ edit" (the editor) were each requested by an earlier round.
* gRPC target has no placeholder: it has one ("localhost:50051 (https:// for TLS)") and
  shows it while the field is empty (the recordings fill the target first).
* "y copy" on the Timing tab: it copies the timing table.
* Log shows the time twice (wall clock and offset): the legend explains both.
* Daylight contrast and the caret on the first letter: as above.

## Not judged

No recording shows the Scripts, Auth, Metadata or Options tabs in detail, the keychain
and file environment sources, the right-hand sidebar or pane zoom; those are covered
by the UI flow tests (15 flows) and the snapshot tests instead. All recordings are
in the dusk theme; the daylight theme appears in the screenshots.
