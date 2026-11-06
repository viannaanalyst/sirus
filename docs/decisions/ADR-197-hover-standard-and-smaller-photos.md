# ADR-197: One hover for lists, smaller sent pictures, and the composer's sparks

- Status: accepted
- Date: 2026-11-06

## Context

Lists hovered in two ways: some had the sliding pill (`hover_track.rs`) at 320 ms, others lit up each row at once. The owner compared four previews and chose a fast slide. Sent pictures at Monocode's 240 px took too much of the conversation.

## Decision

- The pill moves in 130 ms and fades in 110 ms, for every list that has it. The sidebar's "New" and Pull requests rows, its options menu and the Environment menu's main rows join it.
- A lone button has no neighbour to slide from and keeps its plain hover fill. Rows nested under an open Environment entry do too, for now.
- A sent picture is a 96 px square, cropped, with a 12 px radius, so a row of mixed shapes reads evenly. This differs from Monocode on purpose.
- Show more and Show less slide the bubble with the same reveal as the folds (`Disclosure::from_height`).
- The settings page centers its column, so a wide monitor no longer leaves it against the sidebar.
- While the agent works, the robot on the composer throws a few letters at each hop. The line of letters along the edge is gone.
