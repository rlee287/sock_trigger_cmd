# sock_trigger_cmd

`sock_trigger_cmd` listens to a Unix domain socket and maps null-separated keys into commands to execute. It is meant to allow for the execution of a limited set of commands and is not intended as a replacement for remote shells like SSH.

Commands are run directly (i.e. without a shell environment) and only have access to `HOME`, `PATH`, `USER`, `SHELL`, and `TERM`, although other environment variables can be specified in the usual way with the `VAR=VALUE cmd` syntax. If `sock_trigger_cmd` is run as root, commands can be run as other users using the `runuser` command.

The socket returns the following information for each command executed:
 - "C" if the command ran to completion, "S" if the command was terminated by a signal, "F" if the command could not be spawned, and "X" for a non-matching key
 - A single `u8` containing the exit code, if the previous byte was a "C"
 - A single `u8` containing the signal number, if the previous byte was a "S"