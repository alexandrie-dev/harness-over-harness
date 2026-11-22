#!/bin/sh
# Растры эмблемы HoH с прозрачным фоном (HOH-41): из исходника с прозрачностью ui/src/assets/emblem-a.webp
# (696×750, sRGBA; копия site/assets/emblem-a.webp, утверждённая эмблема), только установленный ImageMagick (`convert`).
# Фильтр Lanczos, альфа сохраняется; на каждый размер показа — файлы 1×/1.5×/2×/3× (image-set / srcset).
# Запуск из корня дерева: sh ui/scripts/emblem-assets.sh
set -eu
SRC=ui/src/assets/emblem-a.webp
OUT=ui/src/assets
# имя ширина высота (размер показа в CSS px, поле с запасом по пропорции 696:750) резкость
sets() { cat <<'LIST'
splash 200 216 0
hero 108 116 0
chat 96 104 0
brand 32 34 0.5
fav 32 34 0
notch 19 20 0.4
notchs 15 16 0.4
LIST
}
sets | while read -r name w h sharp; do
  for s in 1 1.5 2 3; do
    pw=$(awk "BEGIN{printf \"%d\", $w*$s+0.5}"); ph=$(awk "BEGIN{printf \"%d\", $h*$s+0.5}")
    tag=$(echo "$s" | sed 's/\.//')
    unsharp=""; [ "$sharp" != 0 ] && unsharp="-unsharp 0x0.6+$sharp+0"
    # shellcheck disable=SC2086
    convert "$SRC" -alpha on -filter Lanczos -resize "${pw}x${ph}" $unsharp -background none -gravity center -extent "${pw}x${ph}" \
      -define png:exclude-chunk=all "$OUT/emblem-$name-${tag}x.png"
  done
done
# Значки окна: квадрат с прозрачными углами, эмблема по центру с полем 5 %.
ICONS=apps/hoh-desktop/icons
for n in 32 48 64 128 256 512; do
  inner=$(awk "BEGIN{printf \"%d\", $n*0.90+0.5}")
  convert "$SRC" -alpha on -filter Lanczos -resize "${inner}x${inner}" -background none -gravity center -extent "${n}x${n}" \
    -define png:exclude-chunk=all "$ICONS/${n}x${n}.png"
done
cp "$ICONS/256x256.png" "$ICONS/icon.png"
