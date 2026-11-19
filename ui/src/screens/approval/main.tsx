import { createRoot } from 'react-dom/client';
import '../../styles/tokens.css';
import { ApprovalWindow } from './ApprovalWindow';

createRoot(document.getElementById('root')!).render(<ApprovalWindow />);
