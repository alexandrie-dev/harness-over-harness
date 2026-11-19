#!/bin/bash
# Снимки для сдачи. headless=new отнимает ~87 px высоты у окна под «хром», поэтому
# просим окно на 87 px выше и обрезаем до 1440x900 / 900x900 (вид страницы тот же, что при viewport 900).
cd "$(dirname "$0")/.." || exit 1
python3 src/build.py >/dev/null
shot(){ # имя ширина якорь
  google-chrome --headless=new --disable-gpu --hide-scrollbars --window-size=$2,987 --virtual-time-budget=9000 \
    --screenshot="$PWD/$1.png" "file://$PWD/index.html#$3" >/dev/null 2>&1
  python3 -c "from PIL import Image;i=Image.open('$PWD/$1.png');i.crop((0,0,$2,900)).save('$PWD/$1.png')"
}
shot main-1440 1440 main; shot firstrun-1440 1440 firstrun; shot main-900 900 main; shot approval-1440 1440 approval
