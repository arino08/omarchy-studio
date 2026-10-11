# AUR packaging

`PKGBUILD` builds `omarchy-studio` from the GitHub release tag and installs the
binary, README, LICENSE, the man page and bash/zsh/fish completions. Once on the AUR, users install with any helper:

```bash
yay -S omarchy-studio      # or: paru -S omarchy-studio
```

## Publishing to the AUR (maintainer, one-time setup)

Requires an [aur.archlinux.org](https://aur.archlinux.org) account with your SSH
public key registered.

```bash
git clone ssh://aur@aur.archlinux.org/omarchy-studio.git aur-omarchy-studio
cd aur-omarchy-studio
cp /path/to/omarchy-studio/packaging/aur/{PKGBUILD,.SRCINFO} .
git add PKGBUILD .SRCINFO
git commit -m "Initial import: omarchy-studio 0.0.1"
git push
```

## Releasing a new version

1. Tag and push the new release on GitHub (e.g. `v0.1.0`).
2. Bump `pkgver` (reset `pkgrel=1`) in `PKGBUILD`.
3. Refresh the checksum:
   ```bash
   updpkgsums                     # rewrites sha256sums from the new tarball
   makepkg --printsrcinfo > .SRCINFO
   ```
4. Test a clean build: `makepkg -f` (should compile, pass `check()`, and package).
5. Commit `PKGBUILD` + `.SRCINFO` here, then copy both into the AUR clone and push.
