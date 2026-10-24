# Issue tracker: GitHub

Issues and specs live in DestinySail/optiscaler-nr-installer.
Use the gh CLI from this repository.

- Create: gh issue create --title "..." --body-file <file>
- Read: gh issue view <number> --json number,title,body,labels,comments
- List: gh issue list --state open --json number,title,body,labels
- Comment: gh issue comment <number> --body-file <file>
- Label: gh issue edit <number> --add-label "..." / --remove-label "..."
- Close: gh issue close <number> --comment "..."

Use UTF-8 files for multiline bodies.
"Publish to the issue tracker" means create a GitHub issue.
"Fetch the relevant ticket" means read the GitHub issue.

## Pull requests as a triage surface

**PRs as a request surface: no.**

## Parent tickets and dependencies

Use native GitHub sub-issues and issue dependencies when available.
Otherwise link children with "Part of #<parent>" and record blockers
with "Blocked by: #<number>". A ticket is unblocked when all blockers close.

## Wayfinding operations

A map is an issue labelled wayfinder:map.
Child tickets use wayfinder:research, wayfinder:prototype,
wayfinder:grilling, or wayfinder:task, and link to the map.
Choose the first open, unassigned child with no open blockers in map order.
Claim with gh issue edit <number> --add-assignee @me.
On resolution, comment with the result, close the child,
and add the result and link to the map's Decisions-so-far.
