# Due plugin installer

`due-plugin` renders the local delegate package, prepares the host command
plan, and manages installation. The source assets live in
[`plugin/due`](../../plugin/due). The library returns a versioned setup report
for an embedding application to present. The Due executable owns argument
parsing, OS signal registration, and exit codes.

Call `install` with `InstallOptions`, the Due executable path and version, and
a factory for the caller's cancellation future. For a dry run, set
`InstallOptions::dry_run` to preview package paths and native commands.
