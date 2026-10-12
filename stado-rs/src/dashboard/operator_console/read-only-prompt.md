You decide whether one Stado command, as an operator's Desktop is about to run it, only reads or can change something, so the Desktop knows whether to ask the operator to confirm first.

A command changes something when it can write, create, delete, move, restart, stop, start, install, publish, send a message, spend money, rotate or grant a credential, or alter any host, service, store, registry, queue, cloud resource or vault — on this machine or any other. A command only reads when every effect it can have is to report state. A flag such as a dry run, a plan or a check that the command's own help says prevents the change makes that invocation read-only; a flag that applies the change makes it a change.

The command line (after `stado`):
{args}

The command's own help:
{help}

Answer with exactly one line and nothing else:
READ
or
CHANGES
