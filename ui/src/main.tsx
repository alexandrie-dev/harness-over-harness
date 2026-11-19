import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { ChatScreen } from './screens/chat/ChatScreen';
import { FirstRun } from './screens/firstrun/FirstRun';
import { useChatStore } from './lib/store';
import './styles/tokens.css';

const FIRST_RUN_DONE = 'hoh:first-run:done';
function needsFirstRun() {
  try { return localStorage.getItem(FIRST_RUN_DONE) !== '1'; }
  catch { return true; }
}
function App() {
  const [firstRun, setFirstRun] = useState(needsFirstRun);
  function finish(prefill?: string) {
    // Only a non-sensitive UI completion hint persists here. Rights and model
    // settings remain host-owned; this flag grants no permissions.
    try { localStorage.setItem(FIRST_RUN_DONE, '1'); } catch { /* unavailable storage: show again next time */ }
    if (prefill) useChatStore.getState().setDraft(prefill);
    setFirstRun(false);
  }
  return firstRun ? <FirstRun onFinish={finish} /> : <ChatScreen />;
}
document.body.classList.add('hoh-page');
createRoot(document.getElementById('root')!).render(<App />);
