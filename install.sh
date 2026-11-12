#!/usr/bin/env bash
# Build and install Maprisma Notifier for the current user (no root needed):
#   ~/.local/bin/maprisma-notifier            the COSMIC applet and the --window
#   ~/.local/bin/maprisma-notifier-tray       the tray icon (KDE, Hyprland)
#   ~/.local/bin/maprisma-notifier-recorder   writes the history (started by the applet or tray)
#   ~/.local/share/applications/…desktop      makes it an applet for the panel, and the window
#   ~/.local/share/icons/hicolor/scalable/apps/…svg  the bell icons
#   ~/.config/autostart/…Tray.desktop         the tray on KDE and Hyprland
# Then add it in COSMIC Settings › Desktop › Panel › Applets.
set -euo pipefail
cd "$(dirname "$0")"
id=io.github.ubruckhaus.CosmicMaprismaNotifier
data="${XDG_DATA_HOME:-$HOME/.local/share}"
bin="$HOME/.local/bin"

cargo build --release
for b in maprisma-notifier maprisma-notifier-tray maprisma-notifier-recorder; do
    install -Dm755 "target/release/$b" "$bin/$b"
done
for icon in "$id-symbolic" "$id-unread-symbolic" "$id-dnd-symbolic"; do
    install -Dm644 "data/icons/$icon.svg" "$data/icons/hicolor/scalable/apps/$icon.svg"
done
# Desktop Exec has two escaping layers: quoted arguments, then string values.
desktop_arg() {
    local v=$1
    v=${v//\\/\\\\}
    v=${v//\"/\\\"}
    v=${v//\`/\\\`}
    v=${v//\$/\\\$}
    v=${v//\%/%%}
    v="\"$v\""
    v=${v//\\/\\\\}
    printf '%s' "$v"
}
app_exec=$(desktop_arg "$bin/maprisma-notifier")
tray_exec=$(desktop_arg "$bin/maprisma-notifier-tray")
mkdir -p "$data/applications" "${XDG_CONFIG_HOME:-$HOME/.config}/autostart"
# Preserve all COSMIC applet metadata and its invocation without --window.
while IFS= read -r line; do
    if [[ $line == Exec=* ]]; then printf 'Exec=%s\n' "$app_exec"; else printf '%s\n' "$line"; fi
done < "data/$id.desktop" > "$data/applications/$id.desktop"
cat > "$data/applications/$id.Window.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Maprisma Notifier
Comment=Notification history, unread count and do not disturb
Comment[de]=Verlauf der Benachrichtigungen, ungelesene Anzahl und Nicht stören
Comment[fr]=Historique des notifications, nombre de non lues et ne pas déranger
Comment[es]=Historial de notificaciones, número sin leer y no molestar
Comment[it]=Cronologia delle notifiche, numero di non lette e non disturbare
Comment[pt]=Histórico de notificações, número de não lidas e não perturbar
Comment[nl]=Meldingsgeschiedenis, aantal ongelezen en niet storen
Comment[pl]=Historia powiadomień, liczba nieprzeczytanych i nie przeszkadzać
Exec=$app_exec --window
Icon=$id-symbolic
Terminal=false
NoDisplay=false
Categories=System;Utility;
DESKTOP
# KDE and Hyprland (its session starts XDG autostart through hyprland-session.target, like the
# other Maprisma trays); COSMIC has the applet.
cat > "${XDG_CONFIG_HOME:-$HOME/.config}/autostart/$id.Tray.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Maprisma Notifier tray
Name[de]=Maprisma Notifier im Systembereich
Name[fr]=Icône de notification Maprisma Notifier
Name[es]=Bandeja de Maprisma Notifier
Name[it]=Area di notifica di Maprisma Notifier
Name[pt]=Bandeja do Maprisma Notifier
Name[nl]=Maprisma Notifier in het systeemvak
Name[pl]=Zasobnik Maprisma Notifier
Exec=$tray_exec
Icon=$id-symbolic
Terminal=false
OnlyShowIn=KDE;Hyprland;
DESKTOP
gtk-update-icon-cache -q -t "$data/icons/hicolor" 2>/dev/null || true
echo "Installed. Add “Maprisma Notifier” in COSMIC Settings › Desktop › Panel › Applets."
