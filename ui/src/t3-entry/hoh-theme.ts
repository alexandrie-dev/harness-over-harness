// Тёмная тема HoH: графитовые поверхности, оранжевая метка активности и светлые действия.
// Вставляется в shared/themePalettes.ts при сборке (vite.ts, themePalettes).
// Цвета стартового экрана повторяют canvas, text, accent и chrome в public/boot-theme.js.

const CANVAS = '#111111';
const CHROME = '#141414';
const PANEL = '#1b1b1a';
const SURFACE = '#20201f';
const RAISED = '#292928';
const LINE = '#ffffff22';
const FG = '#f2f1ee';
const MUTED = '#aaa9a7';
const DIM = '#9e9d9b';
const ACCENT = '#f25623';
const INK = '#141414';
const ROSE = '#ff8499';
const PERIWINKLE = '#a5b4fc';

export const HOH_THEME_COLORS: Readonly<Record<string, string>> = {
  canvas: CANVAS, chrome: CHROME, toolbar: CHROME,
  toolbarForeground: FG, toolbarBorder: LINE, toolbarControl: SURFACE, toolbarControlForeground: FG, toolbarControlHover: RAISED,
  surface: PANEL, surfaceRaised: SURFACE, surfaceOverlay: SURFACE,
  text: FG, textMuted: MUTED, border: LINE, input: '#50504e',
  focus: '#edaa95', accent: ACCENT, accentForeground: INK,
  secondary: SURFACE, secondaryForeground: FG, muted: SURFACE, mutedForeground: MUTED,
  placeholder: DIM, secondaryLabel: DIM, iconMuted: DIM,
  error: '#f95676', errorForeground: '#ff8c9f', errorSurface: '#3d202e',
  warning: ROSE, warningForeground: ROSE, warningSurface: '#352535',
  update: PERIWINKLE, updateForeground: PERIWINKLE, updateSurface: '#272d4a',
  accentSurface: SURFACE, accentSurfaceForeground: FG,
  messageSurface: '#252525', messageForeground: FG, messageAction: FG, messageActionForeground: INK, messageActionHover: '#ffffff',
  codeBackground: SURFACE, codeForeground: FG,
  searchMatchBackground: '#8c6c1f', searchMatchForeground: '#f5f5f5', searchMatchActiveBackground: '#eea52b', searchMatchActiveForeground: '#21180a',
  sidebar: CHROME, sidebarForeground: FG, sidebarMutedForeground: MUTED, sidebarControlSurface: PANEL,
  sidebarRowHover: SURFACE, sidebarRowActive: RAISED, sidebarRowSelected: RAISED, sidebarBorder: LINE,
  terminalBackground: PANEL, terminalForeground: FG, terminalCursor: FG, terminalSelection: '#474745', terminalScrollbar: '#393937', terminalScrollbarHover: '#565654',
};
