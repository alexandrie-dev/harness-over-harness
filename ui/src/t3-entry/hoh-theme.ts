// Тема «HoH» в системе тем T3 (HOH-41 В1): палитра «Эмблема» (design/RULES.md, design/round4/palette.css [data-palette="emblem"],
// design/DESIGN.md «Цвет и текст»). Только роли цвета: шрифт, размеры и радиусы остаются T3. Вставляется в
// shared/themePalettes.ts при сборке (vite.ts, themePalettes); ключи — THEME_COLOR_ROLES, все обязательны.
// Роль T3 → цвет HoH — таблица в T3-MAP.md. Красный — только `warning` («ждёт человека»); `error` (T3: «Удалить», сбой) как у T3.

const CHROME = '#080809';   // рамка и фон окна
const PANEL = '#0e0e10';    // панель, поверхность
const SURFACE = '#141516';  // наведение
const RAISED = '#1b1c1d';   // выбранное, всплывающее
const LINE = '#ffffff14';   // белый 8 %
const FG = '#f2f3f5';
const MUTED = '#a3a3a3';    // DESIGN.md: служебный серый
const DIM = '#8f8f8f';
const BLUE = '#4dacf6';     // «работает», фокус, отправка, выбранное
const RED = '#fe3f4f';      // «ждёт вас»

export const HOH_THEME_COLORS: Readonly<Record<string, string>> = {
  canvas: CHROME, chrome: CHROME, toolbar: CHROME,
  toolbarForeground: FG, toolbarBorder: LINE, toolbarControl: SURFACE, toolbarControlForeground: FG, toolbarControlHover: RAISED,
  surface: PANEL, surfaceRaised: SURFACE, surfaceOverlay: SURFACE,
  text: FG, textMuted: MUTED, border: LINE, input: '#272829',
  focus: BLUE, accent: BLUE, accentForeground: CHROME,
  secondary: SURFACE, secondaryForeground: FG, muted: SURFACE, mutedForeground: MUTED,
  placeholder: DIM, secondaryLabel: DIM, iconMuted: DIM,
  // T3 `error` — их «Удалить» и сбой запроса: как у T3 (красный HoH — только «ждёт вас»).
  error: '#fb414a', errorForeground: '#ff6467', errorSurface: '#301214',
  warning: RED, warningForeground: RED, warningSurface: '#2a1015',
  update: BLUE, updateForeground: BLUE, updateSurface: '#0f2230',
  accentSurface: SURFACE, accentSurfaceForeground: FG,
  messageSurface: RAISED, messageForeground: FG, messageAction: BLUE, messageActionForeground: CHROME, messageActionHover: '#3f9be0',
  codeBackground: SURFACE, codeForeground: FG,
  searchMatchBackground: '#8c6c1f', searchMatchForeground: '#f5f5f5', searchMatchActiveBackground: '#eea52b', searchMatchActiveForeground: '#21180a',
  sidebar: CHROME, sidebarForeground: FG, sidebarMutedForeground: MUTED, sidebarControlSurface: PANEL,
  sidebarRowHover: SURFACE, sidebarRowActive: RAISED, sidebarRowSelected: RAISED, sidebarBorder: LINE,
  terminalBackground: PANEL, terminalForeground: FG, terminalCursor: BLUE, terminalSelection: '#1f3a52', terminalScrollbar: '#272829', terminalScrollbarHover: '#353537',
};
