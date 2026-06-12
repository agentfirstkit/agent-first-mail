# Mailbox Agent Notes

This is an afmail mailbox workspace. Write mailbox-specific agent behavior,
priorities, reply style, escalation rules, and user preferences here.

For afmail operations, follow the installed `agent-first-mail` skill. If the
skill is missing, run `afmail skill install` and restart the agent so it reloads
skills.

Do not manually edit `.afmail/` except `.afmail/templates/`; other files are
machine-managed state. Use `afmail` for message disposition, case membership,
archive changes, push queue changes, and remote effects.
