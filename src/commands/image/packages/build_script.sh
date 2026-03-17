# install all packages inside /deps/ with pacman
mkdir /deps -p
buildDependencies=$(find /deps/ -type f -name "*.pkg.tar.zst")
pacman -Sy
if [ -n "$buildDependencies" ]; then
	pacman -U --needed --noconfirm $buildDependencies
fi
pacman -S --needed --noconfirm sudo # Install sudo
useradd builduser -m # Create the builduser
passwd -d builduser # Delete the buildusers password
printf 'builduser ALL=(ALL) ALL\nDefaults    env_keep += "PKGDEST"\nDefaults    env_keep += "BUILDDIR"\nDefaults    env_keep += "PACMAN"\n' | tee -a /etc/sudoers # Allow the builduser passwordless sudo
cd /src
#rm -rf /out/makepkg/pkg
#rm -rf /out/makepkg/*.pkg.tar.zst
#rm -rf /out/*.pkg.tar.zst
mkdir /out/makepkg -p
chown builduser:builduser /out/ -R

# makepkg can use $PACMAN; keep it as a path (no spaces) to avoid parsing issues.
cat >/usr/local/bin/pacman-no-timeout <<'EOF'
#!/bin/sh
exec /usr/bin/pacman --disable-download-timeout --noconfirm "$@"
EOF
chmod +x /usr/local/bin/pacman-no-timeout
export PACMAN=/usr/local/bin/pacman-no-timeout

sudo -u builduser bash -c 'makepkg --noconfirm --noprogressbar -s -f'
