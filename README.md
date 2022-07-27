# sock_trigger_cmd

`sock_trigger_cmd` listens to a Unix domain socket and maps null-separated keys into commands to execute.

Commands are run directly (i.e. without a shell environment) and do not have access to any preexisting environment variables, although environment variables can be specified in the usual way with the `VAR=VALUE cmd` syntax. If `fifo_trigger_cmd` is run as root, commands can be run as other users using the `runuser` command.