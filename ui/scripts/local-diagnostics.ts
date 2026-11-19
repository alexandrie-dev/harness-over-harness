import type { Plugin } from 'vite';

/** Keep upstream error messages/codes, omit remote diagnostic links from desktop artifacts.
 * Cached packages/integrities stay unchanged. Only this inert diagnostic prefix
 * is rewritten; resource addresses and application configuration are untouched.
 */
export function localDiagnostics(): Plugin {
  return {
    name: 'hoh-local-diagnostics', enforce: 'pre',
    transform(code, id) {
      if (id.includes('/node_modules/marked/') && code.includes('https://github.com/markedjs/marked.'))
        return { code: code.replaceAll('https://github.com/markedjs/marked.', 'the marked issue tracker.'), map: null };
      if (!(id.includes('/node_modules/react/') || id.includes('/node_modules/react-dom/'))) return;
      if (!code.includes('https://react.dev/errors/')) return;
      return { code: code.replaceAll('https://react.dev/errors/', 'React error '), map: null };
    },
  };
}
