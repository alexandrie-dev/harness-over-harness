// Прослойка окна HoH вместо apps/web/src/components/files/InlineAudioPreview.tsx (T3 Code, MIT): обычный <audio>.
export function InlineAudioPreview(props: { readonly src?: string; readonly [key: string]: unknown }) { return <audio controls src={props.src} />; }
