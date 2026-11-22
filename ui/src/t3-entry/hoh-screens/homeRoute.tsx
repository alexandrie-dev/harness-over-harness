// Маршрут `/` окна HoH (HOH-41 В2): вместо routes/_chat.index.tsx T3 (подмена в vite.ts) — главная экрана раунда 8.
import { createFileRoute } from '@tanstack/react-router';
import { HomeScreen } from './HomeScreen';

export const Route = createFileRoute('/_chat/')({ component: HomeScreen });
