# Contributing

**Aloud does not accept outside code contributions.** Issues and bug reports
are very welcome; pull requests will be declined, with thanks.

## Why, since this is unusual

It is a licensing constraint, not a judgement about anyone's code.

Aloud is licensed under [PolyForm Noncommercial 1.0.0](LICENSE). Code you
submitted would arrive licensed on those same noncommercial terms — which
means it could never go into a commercial build of Aloud, including one by
the copyright holder. A single merged pull request would therefore close off
a paid version of the project permanently, for everyone.

The usual fix is a Contributor Licence Agreement, asking every contributor to
grant the maintainer relicensing rights. For a project this size that is more
process than the contributions would be worth, and it asks people to sign
something before they can help. Declining pull requests outright is the more
honest trade.

## What genuinely helps

- **Bug reports.** Especially anything reproducible with the log file
  (macOS `~/Library/Logs/Aloud/aloud.log`, Windows
  `%LOCALAPPDATA%\com.andriileso.aloud\logs\aloud.log`) — read it first, it
  is deliberately verbose about which branch of the pipeline was taken.
- **Platform reports.** Aloud is developed on exactly two machines, an M1 Air
  and one Windows 11 PC. Behaviour on an Intel Mac, on Windows 10, or on an
  unusual multi-monitor or mixed-DPI setup is genuinely unknown, and a
  report from one is more useful here than a patch.
- **Telling us a shortcut is taken.** The selection shortcut is a macOS
  Service key equivalent, and any app can shadow it. If the default collides
  with something common, that is worth knowing — see the README for how to
  inspect what claims a chord on your machine.

## If you want to build on it

Fork it and change what you like — the licence permits that for any
noncommercial purpose, and you are not obliged to send anything back. For
commercial use, contact the copyright holder.
