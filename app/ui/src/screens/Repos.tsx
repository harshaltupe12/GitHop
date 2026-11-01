import { useCallback, useEffect, useRef, useState } from 'react';
import { Avatar } from '../components/Avatar';
import { Banner } from '../components/Banner';
import { Button } from '../components/Button';
import { ErrorText } from '../components/ErrorText';
import { Dialog } from '../components/Dialog';
import { Ico, Icons } from '../components/icons';
import { EmptyState, ScreenHeader } from '../components/Layout';
import { MenuButton } from '../components/Menu';
import { Pill, type PillKind } from '../components/Pill';
import { Segmented } from '../components/Segmented';
import { Spinner } from '../components/Spinner';
import { Table, type Column } from '../components/Table';
import { useToast } from '../components/Toast';
import { Tooltip } from '../components/Tooltip';
import { reposStore, replaceRepo } from '../lib/appState';
import { baseName, normalizePath, profileById, sortProfiles, truncateMiddle } from '../lib/format';
import * as ipc from '../lib/ipc';
import { useNav } from '../lib/nav';
import { KEYS, readJson, removeKey, writeJson } from '../lib/storage';
import type { HookStage, IpcError, ProfileSet, RepoRow } from '../lib/types';
import { S } from '../strings';

type Action = 'fix' | 'husky' | 'pin' | null;
export type RepoStatus = { kind: PillKind; text: string; action: Action };

/**
 * Status priority: unreadable > wrong commits > husky > unassigned > email
 * mismatch > no remote > matches. "Wrong commits" counts only the unpushed ones
 * Fix can change (`unpushed_offenders`); pushed ones are explained on the Fix screen.
 */
export function repoStatus(r: RepoRow, profiles: ProfileSet): RepoStatus {
  if (r.error) return { kind: 'warning', text: S.pill.unreadable, action: null };
  const offenders = r.unpushed_offenders;
  if (offenders > 0)
    return { kind: 'blocked', text: S.pill.wrongCommits(offenders), action: 'fix' };
  if (r.resolution.warnings.some((w) => w.kind === 'local-hooks-path'))
    return { kind: 'warning', text: S.pill.husky, action: 'husky' };
  const pinned = r.resolution.source === 'pin';
  if (!r.resolution.profile_id || !pinned)
    return { kind: 'warning', text: S.pill.unassigned, action: 'pin' };
  if (r.resolution.warnings.some((w) => w.kind === 'pin-owner-disagree'))
    return { kind: 'warning', text: S.pill.pinDisagrees, action: 'pin' };
  const p = profileById(profiles, r.resolution.profile_id);
  const email = r.identity?.author.email;
  if (p && email && email.trim().toLowerCase() !== p.email.trim().toLowerCase())
    return { kind: 'warning', text: S.pill.wrongEmail, action: 'pin' };
  if (!r.resolution.remote) return { kind: 'warning', text: S.pill.noRemote, action: null };
  return { kind: 'ok', text: S.pill.matches, action: null };
}

function mergeRepos(old: RepoRow[], fresh: RepoRow[]): RepoRow[] {
  const key = (r: RepoRow) => normalizePath(r.root).toLowerCase();
  const m = new Map(old.map((r) => [key(r), r]));
  for (const r of fresh) m.set(key(r), r);
  return [...m.values()].sort((a, b) => baseName(a.root).localeCompare(baseName(b.root)));
}

export function ReposScreen({ profiles }: { profiles: ProfileSet }) {
  const nav = useNav();
  const toast = useToast();
  const st = reposStore.useStore();
  const scanToken = useRef(0);
  const [husky, setHusky] = useState<RepoRow | null>(null);
  const [pinningAll, setPinningAll] = useState(false);
  const [error, setError] = useState<IpcError | null>(null);

  const scanFolders = useCallback(async (folders: string[]) => {
    const token = ++scanToken.current;
    setError(null);
    const unlisten = await ipc
      .onScanProgress((p) => {
        if (token === scanToken.current)
          reposStore.set((s) => ({
            ...s,
            scanning: { folder: normalizePath(p.folder), found: p.found },
          }));
      })
      .catch(() => undefined);
    try {
      for (const folder of folders) {
        if (token !== scanToken.current) return;
        reposStore.set((s) => ({ ...s, scanning: { folder, found: 0 } }));
        const r = await ipc.scan(folder);
        if (token !== scanToken.current) return;
        reposStore.set((s) => ({ ...s, repos: mergeRepos(s.repos, r) }));
      }
      reposStore.set((s) => ({ ...s, scanning: null, scanned: true }));
    } catch (e) {
      if (token === scanToken.current) {
        setError(e as IpcError);
        reposStore.set((s) => ({ ...s, scanning: null }));
      }
    } finally {
      unlisten?.();
    }
  }, []);

  const chooseAndScan = useCallback(async () => {
    const picked = await ipc.pickFolder(S.repos.chooseFolder).catch(() => null);
    if (!picked) return;
    const folder = normalizePath(picked);
    const folders = [...new Set([...reposStore.get().folders, folder])];
    reposStore.set((s) => ({ ...s, folders }));
    writeJson(KEYS.scanFolders, folders);
    await scanFolders([folder]);
  }, [scanFolders]);

  // Remembered folders: rescan once per session.
  useEffect(() => {
    const s = reposStore.get();
    if (s.autoScanned) return;
    const remembered = readJson<string[]>(KEYS.scanFolders, []).filter(
      (f) => typeof f === 'string',
    );
    reposStore.set((x) => ({ ...x, autoScanned: true, folders: remembered }));
    if (remembered.length) void scanFolders(remembered);
  }, [scanFolders]);

  useEffect(() => {
    if (nav.intent?.kind === 'scan') {
      nav.clearIntent();
      void chooseAndScan();
    }
  }, [nav, chooseAndScan]);

  const cancel = () => {
    scanToken.current++;
    // Stop the backend walk too (CPU/disk), not only the UI.
    void ipc.cancelScan().catch(() => undefined);
    reposStore.set((s) => ({ ...s, scanning: null, scanned: s.repos.length > 0 }));
    toast({ kind: 'info', message: S.repos.scanCancelled });
  };

  const forget = () => {
    if (reposStore.get().scanning) void ipc.cancelScan().catch(() => undefined);
    scanToken.current++;
    removeKey(KEYS.scanFolders);
    reposStore.set((s) => ({
      ...s,
      folders: [],
      repos: [],
      scanned: false,
      scanning: null,
    }));
  };

  const doPin = async (r: RepoRow, id: string) => {
    try {
      const updated = await ipc.pin(r.root, id);
      replaceRepo(updated);
      toast({ message: S.repos.pinned(baseName(r.root), profileById(profiles, id)?.label ?? id) });
    } catch (e) {
      toast({ kind: 'error', message: (e as IpcError).message });
    }
  };

  const resolvable = st.repos.filter(
    (r) => !r.error && r.resolution.profile_id && r.resolution.source !== 'pin',
  );

  const pinAll = async () => {
    setPinningAll(true);
    try {
      const updated = await ipc.pinAll(resolvable.map((r) => r.root));
      reposStore.set((s) => ({ ...s, repos: mergeRepos(s.repos, updated) }));
      toast({ message: S.repos.pinnedAll(updated.length) });
    } catch (e) {
      toast({ kind: 'error', message: (e as IpcError).message });
    } finally {
      setPinningAll(false);
    }
  };

  const ordered = sortProfiles(profiles);

  const columns: Column<RepoRow>[] = [
    {
      key: 'repo',
      header: S.repos.colRepo,
      className: 'max-w-[16rem] gh-cell-full',
      render: (r) => (
        <div className="flex min-w-0 flex-col">
          <span className="truncate font-mono text-mono-small text-ink" title={r.root}>
            {baseName(r.root)}
          </span>
          <span className="truncate font-mono text-mono-small text-ink-muted" title={r.root}>
            {r.resolution.remote
              ? `${r.resolution.remote.owner}/${r.resolution.remote.repo}`
              : truncateMiddle(r.root, 32)}
          </span>
        </div>
      ),
    },
    {
      key: 'profile',
      header: S.repos.colProfile,
      render: (r) => {
        const p = profileById(profiles, r.resolution.profile_id);
        if (!p) return <span className="text-small text-ink-muted">{S.repos.noProfile}</span>;
        const src = r.resolution.source;
        return (
          <span className="flex flex-wrap items-center gap-2">
            <Avatar label={p.label} color={p.color} size={22} decorative />
            <span>{p.label}</span>
            {src === 'pin' ? (
              <Pill kind="neutral">{S.pill.pinned}</Pill>
            ) : src ? (
              <span className="text-small text-ink-muted">{S.repos.by[src]}</span>
            ) : null}
          </span>
        );
      },
    },
    {
      key: 'status',
      header: S.repos.colStatus,
      render: (r) => {
        const s = repoStatus(r, profiles);
        return (
          <span className="flex flex-wrap items-center gap-1">
            <Pill kind={s.kind}>{s.text}</Pill>
            {r.error ? (
              <Tooltip content={r.error} describe>
                <button
                  type="button"
                  aria-label={S.pill.unreadable}
                  className="inline-flex rounded-sm text-ink-muted"
                >
                  <Ico icon={Icons.info} />
                </button>
              </Tooltip>
            ) : null}
          </span>
        );
      },
    },
    {
      key: 'action',
      header: <span className="sr-only">{S.repos.colAction}</span>,
      align: 'right',
      render: (r) => {
        const s = repoStatus(r, profiles);
        if (s.action === 'fix')
          return (
            <Button
              compact
              icon={Icons.fix}
              onClick={() => nav.go('fix', { kind: 'fix-repo', repo: r.root })}
            >
              {S.repos.fix}
            </Button>
          );
        if (s.action === 'husky')
          return (
            <Button compact icon={Icons.info} onClick={() => setHusky(r)}>
              {S.repos.howToFix}
            </Button>
          );
        if (s.action === 'pin' && ordered.length)
          return (
            <MenuButton
              label={S.repos.pinMenu(baseName(r.root))}
              heading={S.repos.pin}
              triggerClassName="inline-flex min-h-7 items-center gap-1.5 rounded-md border border-line bg-surface px-3 text-body text-ink hover:bg-surface-hover active:bg-surface-pressed"
              trigger={
                <>
                  <Ico icon={Icons.pin} />
                  {S.repos.pin}
                </>
              }
              items={[
                ...ordered.filter((p) => p.id === r.resolution.profile_id),
                ...ordered.filter((p) => p.id !== r.resolution.profile_id),
              ].map((p) => ({
                key: p.id,
                label: S.repos.pinTo(p.label),
                leading: <Avatar label={p.label} color={p.color} size={22} decorative />,
                onSelect: () => void doPin(r, p.id),
              }))}
            />
          );
        return null;
      },
    },
  ];

  const scanning = st.scanning;

  return (
    <div>
      <ScreenHeader
        title={S.repos.title}
        actions={
          <>
            {resolvable.length > 0 && !scanning ? (
              <Button variant="primary" icon={Icons.pin} onClick={pinAll} loading={pinningAll}>
                {S.repos.pinAll}
              </Button>
            ) : null}
            {st.folders.length > 0 && !scanning ? (
              <Button variant="ghost" onClick={forget}>
                {S.repos.forget}
              </Button>
            ) : null}
            {st.folders.length > 0 && !scanning ? (
              <Button onClick={() => void scanFolders(st.folders)}>{S.repos.rescan}</Button>
            ) : null}
            <Button
              icon={Icons.scan}
              onClick={() => void chooseAndScan()}
              disabled={!!scanning}
              aria-keyshortcuts="Control+F"
            >
              {S.repos.scan}
            </Button>
          </>
        }
      />

      <div aria-live="polite" className="mb-3">
        {scanning ? (
          <div className="flex flex-wrap items-center gap-3 text-body text-ink">
            <Spinner className="text-aqua-strong" />
            <span className="font-mono text-mono">
              {S.repos.scanning(scanning.folder, scanning.found)}
            </span>
            <Button compact onClick={cancel}>
              {S.common.cancel}
            </Button>
          </div>
        ) : st.scanned && st.folders.length ? (
          <p className="font-mono text-mono-small text-ink-muted">
            {S.repos.scanned(st.folders, st.repos.length)}
          </p>
        ) : null}
      </div>

      {error ? (
        <Banner
          kind="blocked"
          className="mb-3"
          action={{ label: S.common.retry, onClick: () => void scanFolders(st.folders) }}
        >
          <ErrorText error={error} />
        </Banner>
      ) : null}

      {st.repos.length > 0 ? (
        <Table
          label={S.repos.tableLabel}
          columns={columns}
          rows={st.repos}
          rowKey={(r) => r.root}
        />
      ) : scanning ? null : st.scanned ? (
        <EmptyState icon={Icons.emptyScan} body={S.repos.noRepos} />
      ) : (
        <EmptyState
          icon={Icons.emptyScan}
          body={S.repos.emptyBody}
          action={
            <Button icon={Icons.scan} onClick={() => void chooseAndScan()}>
              {S.repos.scan}
            </Button>
          }
        />
      )}

      <HuskyDialog repo={husky} onClose={() => setHusky(null)} />
    </div>
  );
}

function HuskyDialog({ repo, onClose }: { repo: RepoRow | null; onClose: () => void }) {
  const toast = useToast();
  const [stage, setStage] = useState<HookStage>('commit');
  const [snippet, setSnippet] = useState<string>('');
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!repo) return;
    let live = true;
    setError(null);
    ipc
      .hooksSnippet(stage)
      .then((s) => live && setSnippet(s))
      .catch((e: IpcError) => live && setError(e.message));
    return () => {
      live = false;
    };
  }, [repo, stage]);

  const hooksPath = repo?.resolution.warnings.find((w) => w.kind === 'local-hooks-path');

  return (
    <Dialog
      open={!!repo}
      title={S.repos.huskyTitle}
      onClose={onClose}
      wide
      footer={
        <>
          <Button onClick={onClose}>{S.common.close}</Button>
          <Button
            variant="primary"
            icon={Icons.copy}
            disabled={!snippet}
            onClick={async () => {
              try {
                await ipc.copyText(snippet);
                toast({ message: S.repos.snippetCopied });
              } catch (e) {
                toast({ kind: 'error', message: String(e) });
              }
            }}
          >
            {S.repos.copySnippet}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-3">
        <p>{S.repos.huskyBody}</p>
        {hooksPath && hooksPath.kind === 'local-hooks-path' ? (
          <p className="font-mono text-mono-small text-ink-muted">{hooksPath.path}</p>
        ) : null}
        <Segmented<HookStage>
          label={S.repos.huskyStage}
          value={stage}
          onChange={setStage}
          options={[
            { value: 'commit', label: S.repos.stageCommit },
            { value: 'push', label: S.repos.stagePush },
          ]}
        />
        {error ? (
          <Banner kind="blocked">{error}</Banner>
        ) : (
          <pre className="overflow-x-auto whitespace-pre-wrap break-all rounded-md border border-line bg-bg px-3 py-2 font-mono text-mono text-ink">
            {snippet}
          </pre>
        )}
      </div>
    </Dialog>
  );
}
