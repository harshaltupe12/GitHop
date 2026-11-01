// Fonts are bundled, Latin subset only (Design §3.2, §12.3) — no CDN, no web font requests.
import '@fontsource/ibm-plex-sans/latin-400.css';
import '@fontsource/ibm-plex-sans/latin-500.css';
import '@fontsource/ibm-plex-mono/latin-400.css';
import './styles/index.css';

import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import { TrayMenu } from './tray/TrayMenu';

const root = document.getElementById('root');
if (root) {
  const isTray = window.location.hash === '#tray';
  if (isTray) document.documentElement.classList.add('tray');
  createRoot(root).render(<StrictMode>{isTray ? <TrayMenu /> : <App />}</StrictMode>);
}
