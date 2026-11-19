// Each feature is both parsed and exercised. A computed-color string alone is
// insufficient: verify oklch by comparing a rendered pixel through canvas.
function computed(id) { return getComputedStyle(document.getElementById(id)).color; }
function pixel(color) {
  const context = document.createElement('canvas').getContext('2d');
  context.fillStyle = '#010203';
  context.fillStyle = color;
  context.fillRect(0, 0, 1, 1);
  return [...context.getImageData(0, 0, 1, 1).data].slice(0, 3);
}
const oklchColor = computed('oklch');
const mixColor = computed('mix');
const layerColor = computed('layer');
const gray = pixel(oklchColor);
const oklch = CSS.supports('color', 'oklch(0.6 0 0)') && gray.every(value => Math.abs(value - 128) <= 2);
const mixed = pixel(mixColor);
const colorMix = CSS.supports('color', 'color-mix(in srgb, red, blue)') && mixed[0] >= 127 && mixed[0] <= 128 && mixed[1] === 0 && mixed[2] >= 127 && mixed[2] <= 128;
const layer = layerColor === 'rgb(0, 128, 0)';
const webkitgtkVersion = new URLSearchParams(location.search).get('webkitgtk');
const result = { webkitgtkVersion, pass: oklch && colorMix && layer, oklch, colorMix, layer, computed: { oklchColor, mixColor, layerColor }, pixels: { gray, mixed }, userAgent: navigator.userAgent };
document.getElementById('result').textContent = JSON.stringify(result, null, 2);
document.title = 'HOH_PROBE:' + JSON.stringify(result);
