import { memo, type ReactNode, useMemo } from 'react';
import { Lexer, type Token } from 'marked';

type TableCell = { tokens: Token[] };
type ListItem = { task?: boolean; checked?: boolean; tokens: Token[] };
type WithInline = { tokens?: Token[]; text?: string; raw?: string };
function inline(tokens: Token[] = []): ReactNode[] {
  return tokens.map((token, index) => {
    const child = inline((token as WithInline).tokens);
    switch (token.type) {
      case 'strong': return <strong key={index}>{child}</strong>;
      case 'em': return <em key={index}>{child}</em>;
      case 'del': return <del key={index}>{child}</del>;
      case 'codespan': return <code key={index}>{token.text}</code>;
      case 'br': return <br key={index} />;
      // Remote images never load; links remain inert text in this milestone.
      // Raw HTML, SVG, math, handlers and script URLs never become DOM elements.
      case 'image': return <span key={index}>{token.raw}</span>;
      case 'link': return <span key={index} data-markdown-link="inert">{token.raw}</span>;
      case 'html': return <span key={index}>{token.raw}</span>;
      case 'escape':
      case 'text': return <span key={index}>{child.length ? child : token.text}</span>;
      default: return <span key={index}>{token.raw}</span>;
    }
  });
}
function blocks(tokens: Token[] = []): ReactNode[] {
  return tokens.map((token, index) => <MarkdownBlock key={index} token={token} />);
}
const MarkdownBlock = memo(function MarkdownBlock({ token }: { token: Token }) {
  switch (token.type) {
    case 'space': return null;
    case 'heading': {
      const children = inline(token.tokens);
      // Heading levels are lexer data, never a model-controlled element name.
      switch (token.depth) {
        case 1: return <h2>{children}</h2>;
        case 2: return <h3>{children}</h3>;
        case 3: return <h4>{children}</h4>;
        default: return <h5>{children}</h5>;
      }
    }
    case 'paragraph': return <p>{inline(token.tokens)}</p>;
    case 'text': { const children = inline((token as WithInline).tokens); return <p>{children.length ? children : token.text}</p>; }
    case 'code': return <pre><code>{token.text}</code></pre>;
    case 'blockquote': return <blockquote>{blocks(token.tokens)}</blockquote>;
    case 'hr': return <hr />;
    case 'list': {
      const children = token.items.map((item: ListItem, index: number) => <li key={index}>{item.task && <span aria-label={item.checked ? 'Выполнено' : 'Не выполнено'}>{item.checked ? '☑ ' : '☐ '}</span>}{blocks(item.tokens)}</li>);
      return token.ordered ? <ol start={typeof token.start === 'number' ? token.start : undefined}>{children}</ol> : <ul>{children}</ul>;
    }
    case 'table': return <table><thead><tr>{token.header.map((cell: TableCell, i: number) => <th key={i}>{inline(cell.tokens)}</th>)}</tr></thead><tbody>{token.rows.map((row: TableCell[], i: number) => <tr key={i}>{row.map((cell, j) => <td key={j}>{inline(cell.tokens)}</td>)}</tr>)}</tbody></table>;
    case 'html': return <p data-markdown-html="literal">{token.raw}</p>;
    default: return <p>{token.raw}</p>;
  }
}, (previous, next) => previous.token.type === next.token.type && previous.token.raw === next.token.raw);

export const Markdown = memo(function Markdown({ text }: { text: string }) {
  const tokens = useMemo(() => Lexer.lex(text, { gfm: true }), [text]);
  return <div className="chat-markdown" data-markdown="safe">{blocks(tokens)}</div>;
});
