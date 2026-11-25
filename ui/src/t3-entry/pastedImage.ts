import { invoke } from '@tauri-apps/api/core';

export async function readPastedImage(): Promise<File | null> {
  const image = await invoke<{ dataBase64: string; mimeType: string; name: string } | null>(
    'read_pasted_image',
  );
  if (!image) return null;
  const bytes = Uint8Array.from(atob(image.dataBase64), (character) => character.charCodeAt(0));
  return new File([bytes], image.name, { type: image.mimeType });
}
