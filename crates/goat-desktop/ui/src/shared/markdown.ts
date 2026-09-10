import DOMPurify from 'dompurify';
import { Marked } from 'marked';
import { openUrl } from '@tauri-apps/plugin-opener';

const escape = (text: string) => text.replace(/[&<>"']/g, character => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[character]!);
const parser = new Marked({
  gfm: true,
  breaks: false,
  renderer: { html: token => escape(token.text) },
});

export function markdown(text: string): string {
  return DOMPurify.sanitize(parser.parse(text, { async: false }), {
    ALLOWED_TAGS: ['p', 'br', 'strong', 'em', 'del', 'code', 'pre', 'blockquote', 'ul', 'ol', 'li', 'a', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'hr', 'table', 'thead', 'tbody', 'tr', 'th', 'td', 'input'],
    ALLOWED_ATTR: ['href', 'title', 'start', 'type', 'checked', 'disabled'],
    ALLOW_DATA_ATTR: false,
  });
}

export async function openMarkdownLink(event: MouseEvent): Promise<void> {
  const link = event.target instanceof Element ? event.target.closest('a') : null;
  if (!link) return;
  event.preventDefault();
  const href = link.getAttribute('href');
  if (!href) return;
  const url = new URL(href, 'https://invalid.local');
  if (!['https:', 'http:', 'mailto:'].includes(url.protocol) || url.hostname === 'invalid.local') {
    throw new Error('Only absolute HTTP, HTTPS, and email links can be opened.');
  }
  await openUrl(url.href);
}

export function imageSource(image: { media_type: string; data: string }): string | undefined {
  if (!['image/png', 'image/jpeg', 'image/webp', 'image/gif'].includes(image.media_type)) return undefined;
  return `data:${image.media_type};base64,${image.data}`;
}
