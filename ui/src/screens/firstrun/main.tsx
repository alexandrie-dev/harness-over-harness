// Отдельная страница первого запуска. Основной экран подключит `FirstRun` сам (ui2);
// до этого страница нужна проверкам и просмотру.
import { createRoot } from 'react-dom/client';
import '../../styles/tokens.css';
import { FirstRun } from './FirstRun';

createRoot(document.getElementById('root')!).render(
  <FirstRun onFinish={() => { document.body.dataset.finished = ''; }} />,
);
