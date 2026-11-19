// Shared literal Markdown expectations for Node and the external browser run.
const sources = [
  '<script>globalThis.__markdownRan=true</script>',
  '<img src=x onerror="globalThis.__markdownRan=true">',
  '<svg onload="globalThis.__markdownRan=true"></svg>',
  '[Нажми](javascript:globalThis.__markdownRan=true)',
  '![Картинка](data:image/svg+xml,<svg onload=alert(1)>)',
  '<iframe srcdoc="<script>globalThis.__markdownRan=true</script>"></iframe>',
  '<a href="file:///etc/passwd">Открыть</a>',
  '<form><input autofocus onfocus="globalThis.__markdownRan=true"></form>',
  '<style>body{display:none}</style>',
];
export const harmfulMarkdownCases = sources.map(source => ({
  source,
  expected: source === '<a href="file:///etc/passwd">Открыть</a>' ? '<a href="файл">Открыть</a>' : source,
}));
