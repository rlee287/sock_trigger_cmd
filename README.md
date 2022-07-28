# sock_trigger_cmd

`sock_trigger_cmd` listens to a Unix domain socket and maps null-separated keys into commands to execute.

Commands are run directly (i.e. without a shell environment) and do not have access to any preexisting environment variables, although environment variables can be specified in the usual way with the `VAR=VALUE cmd` syntax. If `fifo_trigger_cmd` is run as root, commands can be run as other users using the `runuser` command.

The socket returns the following information for each command executed:
 - "C" if the command ran to completion, "S" if the command was terminated by a signal, "F" if the command could not be spawned, and "X" for a non-matching key
 - A single `u8` containing the exit code, if the previous byte was a "C"