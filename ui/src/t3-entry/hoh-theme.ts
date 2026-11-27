// Тёмная тема HoH: графитовые поверхности, оранжевая метка активности и светлые действия.
// Вставляется в shared/themePalettes.ts при сборке (vite.ts, themePalettes).
// Цвета стартового экрана повторяют canvas, text, accent и chrome в public/boot-theme.js.

import type { ThemeColors } from '../t3/base/shared/themePalettes';

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

export const HOH_THEME_COLORS: ThemeColors = {
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

// The original palette used with the glass sidebar and its light ribbons.
const PRISM_CANVAS = '#080809';
const PRISM_PANEL = '#0e0e10';
const PRISM_SURFACE = '#141516';
const PRISM_RAISED = '#1b1c1d';
const PRISM_LINE = '#ffffff14';
const PRISM_TEXT = '#f2f3f5';
const PRISM_MUTED = '#a3a3a3';
const PRISM_DIM = '#8f8f8f';
const PRISM_BLUE = '#4dacf6';
const PRISM_RED = '#fe3f4f';

export const MIDNIGHT_PRISM_COLORS: ThemeColors = {
  canvas: PRISM_CANVAS, chrome: PRISM_CANVAS, toolbar: PRISM_CANVAS,
  toolbarForeground: PRISM_TEXT, toolbarBorder: PRISM_LINE, toolbarControl: PRISM_SURFACE,
  toolbarControlForeground: PRISM_TEXT, toolbarControlHover: PRISM_RAISED,
  surface: PRISM_PANEL, surfaceRaised: PRISM_SURFACE, surfaceOverlay: PRISM_SURFACE,
  text: PRISM_TEXT, textMuted: PRISM_MUTED, border: PRISM_LINE, input: '#272829',
  focus: PRISM_BLUE, accent: PRISM_BLUE, accentForeground: PRISM_CANVAS,
  secondary: PRISM_SURFACE, secondaryForeground: PRISM_TEXT, muted: PRISM_SURFACE, mutedForeground: PRISM_MUTED,
  placeholder: PRISM_DIM, secondaryLabel: PRISM_DIM, iconMuted: PRISM_DIM,
  error: '#fb414a', errorForeground: '#ff6467', errorSurface: '#301214',
  warning: PRISM_RED, warningForeground: PRISM_RED, warningSurface: '#2a1015',
  update: PRISM_BLUE, updateForeground: PRISM_BLUE, updateSurface: '#0f2230',
  accentSurface: PRISM_SURFACE, accentSurfaceForeground: PRISM_TEXT,
  messageSurface: PRISM_RAISED, messageForeground: PRISM_TEXT, messageAction: PRISM_BLUE,
  messageActionForeground: PRISM_CANVAS, messageActionHover: '#3f9be0',
  codeBackground: PRISM_SURFACE, codeForeground: PRISM_TEXT,
  searchMatchBackground: '#8c6c1f', searchMatchForeground: '#f5f5f5',
  searchMatchActiveBackground: '#eea52b', searchMatchActiveForeground: '#21180a',
  sidebar: PRISM_CANVAS, sidebarForeground: PRISM_TEXT, sidebarMutedForeground: PRISM_MUTED,
  sidebarControlSurface: PRISM_PANEL, sidebarRowHover: PRISM_SURFACE, sidebarRowActive: PRISM_RAISED,
  sidebarRowSelected: PRISM_RAISED, sidebarBorder: PRISM_LINE,
  terminalBackground: PRISM_PANEL, terminalForeground: PRISM_TEXT, terminalCursor: PRISM_BLUE,
  terminalSelection: '#1f3a52', terminalScrollbar: '#272829', terminalScrollbarHover: '#353537',
};
