import type { ArtifactRecord } from '../types';

export function formatByteLen(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MiB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GiB`;
}

export function extensionForMime(mime: string): string {
  if (!mime) return 'bin';
  const normalized = mime.toLowerCase();
  if (normalized === 'application/pdf') return 'pdf';
  if (normalized === 'application/zip') return 'zip';
  if (normalized === 'application/json') return 'json';
  if (normalized === 'application/octet-stream') return 'bin';
  if (normalized.startsWith('audio/')) return normalized.split('/')[1] ?? 'bin';
  if (normalized.startsWith('video/')) return normalized.split('/')[1] ?? 'bin';
  if (normalized.startsWith('image/')) return normalized.split('/')[1] ?? 'bin';
  if (normalized.startsWith('text/')) return 'txt';
  return 'bin';
}

export function suggestFilename(artifact: ArtifactRecord): string {
  const ext = extensionForMime(artifact.mime);
  const base = `${artifact.type || 'artifact'}-${artifact.artifact_id}`;
  return `${base}.${ext}`.replace(/[^A-Za-z0-9._-]/g, '_');
}

export function base64ToBytes(b64: string): Uint8Array {
  const clean = b64.replace(/\s+/g, '');
  const binary = atob(clean);
  const len = binary.length;
  const bytes = new Uint8Array(len);
  for (let i = 0; i < len; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

export function downloadBinaryArtifact(artifact: ArtifactRecord): void {
  const bytes = base64ToBytes(artifact.content);
  const blob = new Blob([bytes as BlobPart], {
    type: artifact.mime || 'application/octet-stream',
  });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = suggestFilename(artifact);
  document.body.appendChild(anchor);
  anchor.click();
  document.body.removeChild(anchor);
  window.setTimeout(() => URL.revokeObjectURL(url), 0);
}
