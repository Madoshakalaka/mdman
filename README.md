mdman: md manager

# background

`claude code` relies on CLAUDE.md markdown files for custom instructions.

Mutiple projects usually share similar parts of instructions so we store them outside of the project and reference them with `@../Projects/my_dir/common.md` inside the project.

However, people will want to uses claude code github actions too, which also relies on the md files. It's impossible for the github action to access those local docs.

mdman will help keep track of duplicated markdown segments and ensure they are in sync.

# Install

```
mdman install
```

will install as a systemd service in order for it to monitor md changes. 

# Usage

```
mdman copy my_md_dir/SOURCE.md my_project_a/
mdman copy my_md_dir/SOURCE.md my_project_b/
mdman copy my_md_dir/SOURCE.md my_project_c/
```

this will copy the file to the target directory, and mdman will watch for changes to SOURCE.md and synchronize it to my_project_a/SOURCE.md my_project_b/SOURCE.md my_project_c/SOURCE.md and sending a desktop notification when it does so.

other commands

```
mdman list
```

```
mdman untrack my_project_a/foo.md
```

use `mdman --help` to find out
