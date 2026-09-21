import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useI18n } from '../i18n';
import { managementApi } from '../services/managementApi';
import { installedPluginEntries, pluginInstallRequest, pluginUpdateBlock, pluginVersionLoaded, pluginHasUpdate,
  type PluginPlatform, type PluginStoreSnapshot, type PluginUpdateEntry } from '../services/pluginUpdates';

type RuntimePlugins = Parameters<typeof pluginVersionLoaded>[0];
type InstallResult = { version: string; restart_required: boolean };

export function InstalledPluginUpdates({ busy, running, onBusyChange }: {
  busy: boolean; running: boolean; onBusyChange: (busy: boolean) => void;
}) {
  const { t } = useI18n();
  const [snapshot, setSnapshot] = useState<PluginStoreSnapshot | null>(null);
  const [platform, setPlatform] = useState<PluginPlatform | null>(null);
  const [loading, setLoading] = useState(false);
  const [updating, setUpdating] = useState('');
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const requestEpoch = useRef(0);
  const updateLock = useRef(false);
  const pending = useRef<{ id: string; name: string; version: string; restart: boolean } | null>(null);
  const mounted = useRef(true);

  const refresh = useCallback(async () => {
    const epoch = ++requestEpoch.current;
    setLoading(true); setError('');
    try {
      const [catalog, detected, runtime] = await Promise.all([
        managementApi.get<PluginStoreSnapshot>('/plugin-store'),
        invoke<PluginPlatform>('detect_core_platform'),
        pending.current ? managementApi.get<RuntimePlugins>('/plugins') : Promise.resolve(null),
      ]);
      if (!mounted.current || epoch !== requestEpoch.current) return;
      setSnapshot(catalog); setPlatform(detected);
      if (runtime && pending.current) {
        const result = pending.current;
        const loaded = pluginVersionLoaded(runtime, result.id, result.version);
        setNotice(t(loaded ? 'kernel.plugins.loaded' : result.restart ? 'kernel.plugins.restart' : 'kernel.plugins.pending', { name: result.name, version: result.version }));
        if (loaded) pending.current = null;
      }
    } catch (reason) {
      if (mounted.current && epoch === requestEpoch.current) { setSnapshot(null); setError(String(reason)); }
    } finally {
      if (mounted.current && epoch === requestEpoch.current) setLoading(false);
    }
  }, [t]);

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; requestEpoch.current += 1; };
  }, []);

  const update = async (entry: PluginUpdateEntry) => {
    if (!snapshot || !running || busy || updateLock.current) return;
    updateLock.current = true;
    setUpdating(entry.id); setError(''); setNotice(''); onBusyChange(true);
    try {
      const request = pluginInstallRequest(entry, snapshot, platform);
      const result = await managementApi.post<InstallResult>(request.path, request.body, request.options);
      pending.current = { id: entry.id, name: entry.name, version: result.version, restart: result.restart_required };
      if (mounted.current) setNotice(t(result.restart_required ? 'kernel.plugins.restart' : 'kernel.plugins.pending', { name: entry.name, version: result.version }));
      await refresh();
    } catch (reason) {
      if (mounted.current) setError(String(reason));
    } finally {
      updateLock.current = false;
      if (mounted.current) setUpdating('');
      onBusyChange(false);
    }
  };

  const entries = snapshot ? installedPluginEntries(snapshot) : [];
  const unmatched = snapshot?.plugins.filter((entry) => entry.installed && !entries.some((matched) => matched.id === entry.id));
  return <section className="panel installed-plugin-updates">
    <div className="version-source-row">
      <div className="version-source-copy"><strong>{t('kernel.plugins.title')}</strong><span>{t('kernel.plugins.hint')}</span></div>
      <button type="button" className="secondary-button" disabled={!running || busy || loading || Boolean(updating)} onClick={() => void refresh()}>
        {t(loading ? 'common.processing' : 'kernel.versions.check')}
      </button>
    </div>
    {!running && <p>{t('kernel.plugins.start')}</p>}
    {error && <p role="alert">{error}</p>}
    {notice && <p role="status">{notice}</p>}
    {snapshot?.source_errors?.map((source) => <p role="alert" key={source.source_url}>{source.source_name}: {source.message}</p>)}
    {unmatched?.length ? <p>{t('kernel.plugins.unmatched')}</p> : null}
    {snapshot && entries.length === 0 && <p>{t('kernel.plugins.empty')}</p>}
    {entries.map((entry) => {
      const blocked = pluginUpdateBlock(entry, snapshot!, platform);
      return <div className="version-source-row" key={`${entry.source_id}/${entry.id}`}>
        <div className="version-source-copy">
          <strong>{entry.name}</strong>
          <span>{entry.installed_version || '?'} → {entry.version}</span>
          <span title={entry.source_url}>{entry.source_name} · {entry.source_url}</span>
          {blocked && <span>{t(`kernel.plugins.block.${blocked}`)}</span>}
          {blocked === 'revision' && entry.upgrade_block_reason && <span>{entry.upgrade_block_reason}</span>}
        </div>
        <button type="button" className="secondary-button" disabled={!running || busy || loading || Boolean(updating) || Boolean(blocked)} onClick={() => void update(entry)}>
          {updating === entry.id ? t('common.processing') : t(pluginHasUpdate(entry) ? 'kernel.plugins.update' : 'kernel.versions.reinstall')}
        </button>
      </div>;
    })}
  </section>;
}
