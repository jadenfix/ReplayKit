import { useState } from 'react';
import type { ArtifactRecord } from '../types';
import {
  downloadBinaryArtifact,
  formatByteLen,
  suggestFilename,
} from '../utils/binary';

interface ArtifactViewerProps {
  artifacts: ArtifactRecord[];
  selectedSpanId: string | null;
}

export function ArtifactViewer({ artifacts, selectedSpanId }: ArtifactViewerProps) {
  if (!selectedSpanId) {
    return (
      <div className="artifact-viewer">
        <div className="artifact-viewer__empty">Select a span to view its artifacts</div>
      </div>
    );
  }

  if (artifacts.length === 0) {
    return (
      <div className="artifact-viewer">
        <div className="artifact-viewer__empty">No artifacts for this span</div>
      </div>
    );
  }

  return (
    <div className="artifact-viewer" data-testid="artifact-viewer">
      {artifacts.map(a => (
        <ArtifactPanel key={a.artifact_id} artifact={a} />
      ))}
    </div>
  );
}

function ArtifactPanel({ artifact }: { artifact: ArtifactRecord }) {
  return (
    <div className={`artifact-panel artifact-panel--${classify(artifact.mime)}`}>
      <div className="artifact-panel__header">
        <span className="artifact-panel__type">{artifact.type}</span>
        <span className="artifact-panel__mime">{artifact.mime}</span>
        {artifact.summary && (
          <span className="artifact-panel__summary">{artifact.summary}</span>
        )}
      </div>
      <div className="artifact-panel__body">
        <ArtifactContent artifact={artifact} />
      </div>
    </div>
  );
}

function ArtifactContent({ artifact }: { artifact: ArtifactRecord }) {
  const mime = artifact.mime;
  const isBase64 = artifact.content_encoding === 'base64';

  if (isBase64 && mime.startsWith('image/')) {
    return (
      <div className="artifact-content artifact-content--image">
        <img
          alt={`${artifact.type} preview`}
          src={`data:${mime};base64,${artifact.content}`}
        />
      </div>
    );
  }

  if (isBase64) {
    return <BinaryArtifactView artifact={artifact} />;
  }

  if (mime === 'application/json') {
    return <JsonViewer content={artifact.content} />;
  }
  if (mime === 'text/x-diff') {
    return <DiffViewer content={artifact.content} />;
  }
  if (mime === 'text/x-shell-log') {
    return <ShellLogViewer content={artifact.content} />;
  }
  // Default: plain text / code
  return <pre className="artifact-content artifact-content--text">{artifact.content}</pre>;
}

function JsonViewer({ content }: { content: string }) {
  let formatted: string;
  try {
    formatted = JSON.stringify(JSON.parse(content), null, 2);
  } catch {
    formatted = content;
  }
  return <pre className="artifact-content artifact-content--json">{formatted}</pre>;
}

function DiffViewer({ content }: { content: string }) {
  const lines = content.split('\n');
  return (
    <pre className="artifact-content artifact-content--diff">
      {lines.map((line, i) => {
        let cls = '';
        if (line.startsWith('+') && !line.startsWith('+++')) cls = 'diff-add';
        else if (line.startsWith('-') && !line.startsWith('---')) cls = 'diff-remove';
        else if (line.startsWith('@@')) cls = 'diff-hunk';
        else if (line.startsWith('---') || line.startsWith('+++')) cls = 'diff-file';
        return <span key={i} className={cls}>{line}{'\n'}</span>;
      })}
    </pre>
  );
}

function ShellLogViewer({ content }: { content: string }) {
  const lines = content.split('\n');
  return (
    <pre className="artifact-content artifact-content--shell">
      {lines.map((line, i) => {
        let cls = '';
        if (line.includes('FAILED')) cls = 'shell-fail';
        else if (line.includes('ok')) cls = 'shell-ok';
        else if (line.startsWith('----') || line.startsWith('failures:')) cls = 'shell-section';
        return <span key={i} className={cls}>{line}{'\n'}</span>;
      })}
    </pre>
  );
}

function BinaryArtifactView({ artifact }: { artifact: ArtifactRecord }) {
  const [showRaw, setShowRaw] = useState(false);
  const sizeLabel = formatByteLen(artifact.byte_len);
  const filename = suggestFilename(artifact);

  return (
    <div className="artifact-content artifact-content--binary" data-testid="artifact-binary">
      <dl className="artifact-content__metadata">
        <dt>MIME</dt>
        <dd>{artifact.mime || 'application/octet-stream'}</dd>
        <dt>Size</dt>
        <dd>{sizeLabel}</dd>
        {artifact.sha256 && (
          <>
            <dt>SHA-256</dt>
            <dd className="artifact-content__hash">{artifact.sha256}</dd>
          </>
        )}
        <dt>Encoding</dt>
        <dd>base64 (transport)</dd>
      </dl>
      <div className="artifact-content__actions">
        <button
          type="button"
          className="artifact-content__download"
          onClick={() => downloadBinaryArtifact(artifact)}
          data-testid="artifact-download"
        >
          Download {filename}
        </button>
        <button
          type="button"
          className="artifact-content__toggle-raw"
          onClick={() => setShowRaw(v => !v)}
        >
          {showRaw ? 'Hide raw base64' : 'Show raw base64'}
        </button>
      </div>
      {showRaw && (
        <pre className="artifact-content artifact-content--text">{artifact.content}</pre>
      )}
    </div>
  );
}

function classify(mime: string): string {
  if (mime.includes('json')) return 'json';
  if (mime.includes('diff')) return 'diff';
  if (mime.includes('shell')) return 'shell';
  if (mime.includes('rust')) return 'code';
  return 'text';
}
