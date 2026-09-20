import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useI18n } from '../i18n';

type Settings = {
  manifestUrl: string;
  installed: {
    repository: string;
    tag: string;
    commit: string;
    binarySha256: string;
    backupDir: string;
  } | null;
};

export function ReviewedCoreChannel({ busy, onChange }: { busy: boolean; onChange: () => void }) {
  const { t } = useI18n();
  const [settings, setSettings] = useState<Settings | null>(null);
  const [url, setUrl] = useState('');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');

  useEffect(() => {
    let cancelled = false;
    void invoke<Settings>('get_reviewed_core_settings').then((value) => {
      if (cancelled) return;
      setSettings(value);
      setUrl(value.manifestUrl);
    }).catch((reason) => { if (!cancelled) setError(String(reason)); });
    return () => { cancelled = true; };
  }, [busy]);

  const save = async (value: string) => {
    setSaving(true);
    setError('');
    try {
      const next = await invoke<Settings>('set_reviewed_core_manifest', { url: value });
      setSettings(next);
      setUrl(next.manifestUrl);
      onChange();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setSaving(false);
    }
  };

  return <div className="version-source-row reviewed-core-channel">
    <div className="version-source-copy">
      <strong>{t('kernel.reviewed.title')}</strong>
      <span>{settings?.manifestUrl ? t('kernel.reviewed.active') : t('kernel.reviewed.official')}</span>
      <span>{t('kernel.reviewed.hint')}</span>
      {settings?.manifestUrl && <span role="note">{t('kernel.reviewed.appGuard')}</span>}
      {settings?.installed && <span title={`SHA-256: ${settings.installed.binarySha256}\n${t('kernel.reviewed.backup')}: ${settings.installed.backupDir}`}>
        {t('kernel.reviewed.installed')}: {settings.installed.repository} · {settings.installed.tag} · {settings.installed.commit.slice(0, 12)}
      </span>}
      {error && <span role="alert">{error}</span>}
    </div>
    <div className="version-source-control">
      <label>
        <span className="sr-only">{t('kernel.reviewed.url')}</span>
        <input type="url" aria-label={t('kernel.reviewed.url')} placeholder="https://…/reviewed-core.json"
          value={url} onChange={(event) => setUrl(event.currentTarget.value)} disabled={busy || saving} />
      </label>
      <button type="button" className="primary-button" disabled={busy || saving || !settings || !url.trim()}
        onClick={() => void save(url.trim())}>{t('kernel.reviewed.select')}</button>
      {settings?.manifestUrl && <button type="button" className="secondary-button" disabled={busy || saving}
        onClick={() => void save('')}>{t('kernel.reviewed.useOfficial')}</button>}
    </div>
  </div>;
}
