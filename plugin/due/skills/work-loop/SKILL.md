---
name: work-loop
description: Carry out a larger order (a backlog of issues, one hard change with independent review, recurring upkeep) by splitting it into pieces, running each piece through Due workers, and continuing without the human except for decisions that cannot be undone or are out of scope.
---

# Run an order through Due workers

Use this when a person gives you an order larger than one assignment and
expects you to keep working: "land these issues", "make this change and have
it reviewed", "keep these projects healthy". You are the caller. You split the
order into pieces, run each piece through a Due worker in its own workspace,
check every result yourself, and keep a board and an inbox on disk. You ask
the person only for decisions that cannot be undone or are out of scope. For a
single assignment, use the `delegate` skill instead.

The procedure ships inside the installed `due` executable. Read it before you
start, and follow it:

```sh
due skills show work-loop
```

If you find a work directory with a `board.md` for the order, you are
continuing someone else's order: read the guide's "Continuing after an
interruption" section first.

If `due skills` does not list `work-loop`, the installed build predates it.
Tell the person, and use the `delegate` skill for single assignments until
Due is updated. Do not improvise the procedure from memory.

The same rules as for `delegate` apply: you own the scope and the acceptance,
a worker's report is evidence to check, and delegation grants no authority
the person did not give you.
