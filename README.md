# AtlasOS Launcher

The Start menu and search of [AtlasOS](https://github.com/EternalCoder454/atlasos):

- **Meta**, or the dock button, opens a blurred panel above the dock. It
  holds your pinned apps, recent files and apps, every app from A to Z, and
  your account and power controls.
- **Typing** turns it into one ranked list of apps, Settings pages, files and
  folders, calculations and unit conversions, commands, web search and every
  installed KRunner plugin.
- **Alt+Space** opens the same search on its own, in the middle of the screen.

It learns what you pick in a plain file you can clear
(`~/.local/state/atlas-launcher/usage.tsv`), and it uses no network unless
you choose to search the web.

How it is built and why: [docs/DESIGN.md](docs/DESIGN.md). Building:
[CLAUDE.md](CLAUDE.md), "Commands".

MIT licence.
