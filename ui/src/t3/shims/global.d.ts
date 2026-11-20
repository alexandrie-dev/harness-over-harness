// Мост Electron из T3 Code в окне HoH отсутствует; тип нужен только перенесённому коду.
interface Window { desktopBridge?: { readMedia(src: string): Promise<{ status: number; data: ArrayLike<number>; mimeType: string } | null> } }
