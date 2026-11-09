# Security policy

GitHop runs on your PC, changes your git configuration and works with your SSH keys, so security reports are taken seriously.

## Supported versions

Security fixes go into the latest release. Please update to it before reporting.

## Reporting a vulnerability

**Please don't open a public issue for security problems.**

Report it privately instead: open the repository's **Security** tab and click **Report a vulnerability**. Only the maintainer can see the report.

Please include:

- What the problem is and what an attacker could do with it
- Steps to reproduce it, and your Windows and Git versions
- Any ideas you have for a fix

You'll get a reply within a few days. Once a fix is released, you'll be credited in the release notes unless you'd rather stay anonymous.

## What GitHop promises

- **No network code.** GitHop never makes network requests. The only traffic is git and ssh doing your own pushes and pulls, and `ssh -T` when you click Test connection.
- **No secrets stored.** Only the paths to your SSH keys. A key passphrase you type in GitHop is used once in memory to hand the key to the Windows SSH agent, and is never saved or logged.
- **Everything is reversible.** Your git config is backed up before every change, and uninstalling restores it.

Anything that breaks one of these promises counts as a security issue.
