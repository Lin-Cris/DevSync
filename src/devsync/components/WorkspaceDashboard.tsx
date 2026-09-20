import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import {
  FiActivity, FiAlertCircle, FiBox, FiCheck, FiCheckCircle, FiChevronDown, FiCircle, FiClock,
  FiCode, FiFolder, FiLoader, FiMinus, FiPlus, FiRefreshCw, FiShield, FiSliders,
  FiSmartphone, FiTool, FiWifi, FiX,
} from "react-icons/fi";
import { toast } from "sonner";
import { devsyncApi } from "../api";
import type { ActivityEntry, BackgroundServiceStatus, DeploymentState, DeploymentUpdate, DeviceSelection, DevSyncDevice, Workspace, WorkspaceInspection } from "../types";
import "./WorkspaceDashboard.css";

type Page = "project" | "devices" | "signing" | "settings";
type Details = Record<string, WorkspaceInspection>;
type AuroraMode = "watching" | "building" | "signing" | "installing" | "waiting" | "ready" | "failed";
const fail = (error: unknown) => error && typeof error === "object" && "message" in error ? String(error.message) : String(error);

export const WorkspaceDashboard = () => {
  const [page, setPage] = useState<Page>("project");
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [selectedWorkspaceId, setSelectedWorkspaceId] = useState<string>();
  const [details, setDetails] = useState<Details>({});
  const [devices, setDevices] = useState<DevSyncDevice[]>([]);
  const [selection, setSelection] = useState<DeviceSelection>({ schemaVersion: 1 });
  const [login, setLogin] = useState(false);
  const [backgroundService, setBackgroundService] = useState<BackgroundServiceStatus>({ enabled: false, running: false, state: "stopped" });
  const [loading, setLoading] = useState(true);
  const [building, setBuilding] = useState<string | null>(null);
  const [syncing, setSyncing] = useState<string | null>(null);
  const [updates, setUpdates] = useState<Record<string, DeploymentUpdate>>({});
  const [cache, setCache] = useState<number>();
  const deviceRefreshInFlight = useRef(false);

  const inspect = useCallback((inspection: WorkspaceInspection) => {
    setWorkspaces((current) => current.some((item) => item.id === inspection.workspace.id) ? current.map((item) => item.id === inspection.workspace.id ? inspection.workspace : item) : [...current, inspection.workspace]);
    setDetails((current) => ({ ...current, [inspection.workspace.id]: inspection }));
    setSelectedWorkspaceId((current) => current ?? inspection.workspace.id);
  }, []);

  const refreshDetails = useCallback(async (items: Workspace[]) => {
    await Promise.all(items.map(async (workspace) => {
      const detail = await devsyncApi.refreshWorkspace(workspace.id).catch(() => undefined);
      if (detail) inspect(detail);
    }));
  }, [inspect]);

  const reload = useCallback(async () => {
    setLoading(true);
    try {
      const [items, activeWorkspaceId] = await Promise.all([
        devsyncApi.listWorkspaces(),
        devsyncApi.getActiveWorkspace(),
      ]);
      setWorkspaces(items);
      setSelectedWorkspaceId((current) => activeWorkspaceId && items.some((item) => item.id === activeWorkspaceId)
        ? activeWorkspaceId
        : current && items.some((item) => item.id === current) ? current : items[0]?.id);
      setLoading(false);
      devsyncApi.getBuildCacheUsage().then((usage) => setCache(usage.bytes)).catch(() => undefined);
      void refreshDetails(items);
    } catch (error) {
      toast.error(`Unable to load projects: ${fail(error)}`);
      setLoading(false);
    }
  }, [refreshDetails]);

  const refreshWorkspaces = useCallback(async () => {
    const [items, activeWorkspaceId] = await Promise.all([
      devsyncApi.listWorkspaces(),
      devsyncApi.getActiveWorkspace(),
    ]).catch(() => [undefined, undefined] as const);
    if (!items) return;
    setWorkspaces(items);
    setSelectedWorkspaceId((current) => activeWorkspaceId && items.some((item) => item.id === activeWorkspaceId)
      ? activeWorkspaceId
      : current && items.some((item) => item.id === current) ? current : items[0]?.id);
  }, []);

  const refreshDevices = useCallback(async (notifyOnError = true) => {
    if (deviceRefreshInFlight.current) return;
    deviceRefreshInFlight.current = true;
    try {
      const [all, saved] = await Promise.all([devsyncApi.listDevices(), devsyncApi.getDeviceSelection()]);
      setDevices(all); setSelection(saved);
      if (!saved.selectedDeviceId && all.length === 1) setSelection(await devsyncApi.selectDevice(all[0]));
    } catch (error) {
      setDevices([]); if (notifyOnError) toast.error(`Unable to load devices: ${fail(error)}`);
    } finally { deviceRefreshInFlight.current = false; }
  }, []);

  useEffect(() => { void reload(); void refreshDevices(); devsyncApi.getLaunchAtLogin().then(setLogin).catch(() => undefined); }, [refreshDevices, reload]);
  useEffect(() => { const timer = window.setInterval(() => void refreshWorkspaces(), 2500); return () => window.clearInterval(timer); }, [refreshWorkspaces]);
  useEffect(() => { const timer = window.setInterval(() => void refreshDevices(false), 15000); return () => window.clearInterval(timer); }, [refreshDevices]);
  useEffect(() => {
    let active = true;
    const refresh = () => devsyncApi.getBackgroundServiceStatus().then((status) => { if (active) setBackgroundService(status); }).catch(() => undefined);
    refresh(); const timer = window.setInterval(refresh, 5000);
    return () => { active = false; window.clearInterval(timer); };
  }, []);
  useEffect(() => {
    let off: (() => void) | undefined;
    listen<DeploymentUpdate>("devsync-deployment", (event) => { setUpdates((current) => ({ ...current, [event.payload.workspaceId]: event.payload })); void refreshWorkspaces(); }).then((cleanup) => { off = cleanup; });
    return () => off?.();
  }, [refreshWorkspaces]);

  const add = async () => {
    const folder = await open({ directory: true, multiple: false, title: "Add Project" });
    if (!folder || Array.isArray(folder)) return;
    try { inspect(await devsyncApi.addWorkspace(folder)); setPage("project"); } catch (error) { toast.error(`Unable to add project: ${fail(error)}`); }
  };
  const activateWorkspace = async (workspaceId: string) => {
    try {
      const activeWorkspaceId = await devsyncApi.selectWorkspace(workspaceId);
      setSelectedWorkspaceId(activeWorkspaceId);
      setPage("project");
    } catch (error) {
      toast.error(`Unable to switch project: ${fail(error)}`);
    }
  };
  const chooseDevice = async (id: string) => { const device = devices.find((item) => item.id === id); if (device) setSelection(await devsyncApi.selectDevice(device)); };
  const sync = async (workspace: Workspace) => {
    setSelectedWorkspaceId(workspace.id); setPage("project"); setSyncing(workspace.id);
    setUpdates((current) => ({ ...current, [workspace.id]: { workspaceId: workspace.id, state: "preparing", message: "Preparing sync" } }));
    try {
      const result = await devsyncApi.deployWorkspace(workspace.id);
      inspect({ workspace: result.workspace, containers: details[workspace.id]?.containers ?? [], metadata: details[workspace.id]?.metadata });
      result.state === "installed" ? toast.success("Sync complete") : toast.error(result.diagnostic ?? stateLabel(result.state));
    } catch (error) { toast.error(`Sync failed: ${fail(error)}`); } finally { setSyncing(null); void refreshWorkspaces(); }
  };
  const build = async (workspace: Workspace) => {
    setBuilding(workspace.id);
    try {
      const result = await devsyncApi.buildWorkspace(workspace.id);
      inspect({ workspace: result.workspace, containers: details[workspace.id]?.containers ?? [], metadata: details[workspace.id]?.metadata });
      result.succeeded ? toast.success("Build succeeded") : toast.error(result.diagnostic ?? "Build failed");
    } catch (error) { toast.error(`Build failed: ${fail(error)}`); } finally { setBuilding(null); void refreshWorkspaces(); }
  };
  const remove = async (workspace: Workspace) => {
    if (!confirm(`Remove ${workspace.displayName} from DevSync? The project will not be deleted.`)) return;
    try {
      await devsyncApi.removeWorkspace(workspace.id);
      const activeWorkspaceId = await devsyncApi.getActiveWorkspace();
      setWorkspaces((current) => current.filter((item) => item.id !== workspace.id));
      setSelectedWorkspaceId(activeWorkspaceId ?? undefined);
    } catch (error) { toast.error(`Unable to remove project: ${fail(error)}`); }
  };
  const setBackground = async (enabled: boolean) => {
    setBackgroundService((current) => ({ ...current, state: "starting" }));
    try { setBackgroundService(enabled ? await devsyncApi.enableBackgroundService() : await devsyncApi.disableBackgroundService()); }
    catch (error) { toast.error(`Unable to ${enabled ? "enable" : "disable"} Automatic signing: ${fail(error)}`); devsyncApi.getBackgroundServiceStatus().then(setBackgroundService).catch(() => undefined); }
  };

  const project = workspaces.find((item) => item.id === selectedWorkspaceId) ?? workspaces[0];
  const device = devices.find((item) => item.id === selection.selectedDeviceId && isConnected(item));
  const connectedCount = devices.filter(isConnected).length;
  return <div className="aurora-window">
    <div className="window-chrome" data-tauri-drag-region><div className="traffic-lights">
      <button className="traffic close" aria-label="Close window" onClick={() => void getCurrentWindow().close()}><FiX /></button>
      <button className="traffic minimize" aria-label="Minimize window" onClick={() => void getCurrentWindow().minimize()}><FiMinus /></button>
      <button className="traffic maximize" aria-label="Maximize window" onClick={() => void getCurrentWindow().toggleMaximize()}><FiPlus /></button>
    </div></div>
    <aside className="aurora-sidebar">
      <div className="aurora-brand"><div><b>DevSync</b><span>Code changes.<br />On your device.</span></div></div>
      <nav className="aurora-nav">
        <Nav active={page === "project"} icon={<FiFolder />} text="Project" go={() => setPage("project")} />
        <Nav active={page === "devices"} icon={<FiSmartphone />} text="Devices" go={() => setPage("devices")} />
        <Nav active={page === "signing"} icon={<FiShield />} text="Signing" go={() => setPage("signing")} />
        <Nav active={page === "settings"} icon={<FiSliders />} text="Settings" go={() => setPage("settings")} />
      </nav>
      <div className="sidebar-caption"><span>Build. Sign. Install.</span><span>Automatically.</span></div>
    </aside>
    <main className="aurora-content">
      {page === "project" && <ProjectView project={project} projects={workspaces} inspection={project ? details[project.id] : undefined} device={device} selection={selection} devices={devices} loading={loading} building={building === project?.id} syncing={syncing === project?.id} update={project ? updates[project.id] : undefined} backgroundService={backgroundService} add={add} selectProject={activateWorkspace} chooseDevice={chooseDevice} setBackground={setBackground} sync={sync} build={build} remove={remove} inspect={inspect} refresh={reload} />}
      {page === "devices" && <Devices devices={devices} selection={selection} choose={chooseDevice} refresh={refreshDevices} />}
      {page === "signing" && <Signing projects={workspaces} backgroundService={backgroundService} setBackground={setBackground} />}
      {page === "settings" && <Settings login={login} cache={cache} backgroundService={backgroundService} setBackground={setBackground} setLogin={async (enabled) => { try { setLogin(await devsyncApi.setLaunchAtLogin(enabled)); } catch (error) { toast.error(`Unable to update setting: ${fail(error)}`); } }} clean={async () => { if (!confirm("Clean DevSync-managed build cache? Your next sync will rebuild.")) return; try { setCache((await devsyncApi.cleanBuildCache()).bytes); toast.success("Build cache cleaned"); } catch (error) { toast.error(`Unable to clean cache: ${fail(error)}`); } }} />}
    </main>
    <footer className="aurora-footer"><span><i className="status-dot green" />{project ? footerStatus(project, device) : "Add a project to begin"}</span><span><FiWifi /> {connectedCount} connected</span><span><FiCheckCircle /> Signing renews automatically</span></footer>
  </div>;
};

const Nav = ({ active, icon, text, go }: { active: boolean; icon: ReactNode; text: string; go: () => void }) => <button className={active ? "active" : ""} onClick={go}>{icon}<span>{text}</span></button>;

const ProjectView = ({ project, projects, inspection, device, selection, devices, loading, building, syncing, update, backgroundService, add, selectProject, chooseDevice, setBackground, sync, build, remove, inspect, refresh }: { project?: Workspace; projects: Workspace[]; inspection?: WorkspaceInspection; device?: DevSyncDevice; selection: DeviceSelection; devices: DevSyncDevice[]; loading: boolean; building: boolean; syncing: boolean; update?: DeploymentUpdate; backgroundService: BackgroundServiceStatus; add: () => void; selectProject: (id: string) => void | Promise<void>; chooseDevice: (id: string) => void; setBackground: (enabled: boolean) => void; sync: (workspace: Workspace) => void; build: (workspace: Workspace) => void; remove: (workspace: Workspace) => void; inspect: (inspection: WorkspaceInspection) => void; refresh: () => void }) => {
  const [detailsOpen, setDetailsOpen] = useState(false);
  const [projectMenuOpen, setProjectMenuOpen] = useState(false);
  const [manageOpen, setManageOpen] = useState(false);
  const state = project ? deriveState(project, update, building, syncing, device) : emptyState();
  const signingStatus = project ? signingSummary(project) : { label: "No profile inspected", tone: "muted", detail: "Build a project to inspect signing." };
  if (loading && !project) return <div className="empty-state"><FiLoader className="spin" /><p>Loading DevSync…</p></div>;
  if (!project) return <div className="empty-state"><FiFolder /><h2>Add your first project</h2><p>Choose an existing Xcode project or workspace to begin.</p><button className="aurora-button" onClick={add}><FiPlus /> Add Project</button></div>;
  return <><header className="project-header"><div><span className="eyebrow">PROJECT</span><div className="project-picker-row"><div className="project-picker-wrap"><button className="project-picker-button" aria-label="Current project" aria-expanded={projectMenuOpen} aria-haspopup="menu" onClick={() => setProjectMenuOpen((open) => !open)}><span>{project.displayName}</span><FiChevronDown /></button>{projectMenuOpen && <ProjectMenu projects={projects} activeId={project.id} selectProject={selectProject} add={add} manage={() => { setProjectMenuOpen(false); setManageOpen(true); }} close={() => setProjectMenuOpen(false)} />}</div><button className="ghost-button add-project-button" onClick={() => void add()}><FiPlus /> Add Project</button></div><p className="project-path">{compactPath(project.folderPath)}</p></div><div className="watching-label"><span className={`watch-ring ${state.mode === "watching" ? "active" : ""}`} />{state.header}</div></header>
    {manageOpen && <ProjectManager projects={projects} activeId={project.id} selectProject={selectProject} remove={remove} close={() => setManageOpen(false)} />}
    <div className="project-grid"><section className="state-column"><div className={`aurora-field mode-${state.mode}`}><div className="aurora-orb"><div className="orb-core" /><div className="orb-shine" /></div><div className="state-copy"><h1>{state.title}</h1><p>{state.description}</p></div></div><StepProgress mode={state.mode} /><div className="project-actions"><button className="ghost-button" onClick={() => setDetailsOpen((open) => !open)}><FiTool /> Project details</button><button className="aurora-button" disabled={!isSyncReady(project) || building || syncing} onClick={() => sync(project)}>{syncing ? <FiLoader className="spin" /> : <FiActivity />} {syncing ? "Syncing…" : "Sync Now"}</button></div>{detailsOpen && <ProjectDetails workspace={project} inspection={inspection} building={building} syncing={syncing} inspect={inspect} build={build} remove={remove} refresh={refresh} />}</section><aside className="right-rail"><DeviceCard device={device} selected={selection.selectedDeviceId} devices={devices} choose={chooseDevice} /><SigningCard status={signingStatus} enabled={backgroundService.enabled} setBackground={setBackground} /><ActivityCard workspace={project} /><p className="rail-note">A faster path from idea to device.</p></aside></div></>;
};

const ProjectMenu = ({ projects, activeId, selectProject, add, manage, close }: { projects: Workspace[]; activeId: string; selectProject: (id: string) => void | Promise<void>; add: () => void; manage: () => void; close: () => void }) => <div className="project-menu" role="menu">
  {projects.map((item) => <button key={item.id} role="menuitem" className="project-menu-item" onClick={() => { close(); if (item.id !== activeId) void selectProject(item.id); }}><span>{item.displayName}</span>{item.id === activeId && <FiCheck />}</button>)}
  <div className="project-menu-separator" />
  <button role="menuitem" className="project-menu-item project-menu-action" onClick={() => { close(); void add(); }}><FiPlus /><span>Connect Project…</span></button>
  <button role="menuitem" className="project-menu-item project-menu-action" onClick={manage}><FiTool /><span>Manage Projects…</span></button>
</div>;

const ProjectManager = ({ projects, activeId, selectProject, remove, close }: { projects: Workspace[]; activeId: string; selectProject: (id: string) => void | Promise<void>; remove: (workspace: Workspace) => void; close: () => void }) => <section className="project-manager"><div className="project-manager-heading"><div><span className="eyebrow">PROJECTS</span><h2>Manage Projects</h2></div><button className="ghost-button" onClick={close}>Done</button></div><div className="project-manager-list">{projects.map((item) => <div className="project-manager-row" key={item.id}><div><b>{item.displayName}{item.id === activeId && <span className="project-active-badge">Active</span>}</b><small>{compactPath(item.folderPath)}</small></div><div className="project-manager-actions"><button className="ghost-button" disabled={item.id === activeId} onClick={() => void selectProject(item.id)}>Switch</button><button className="danger-button" onClick={() => remove(item)}>Disconnect</button></div></div>)}</div></section>;

const StepProgress = ({ mode }: { mode: AuroraMode }) => { const steps = [{ label: "Watch", sub: mode === "watching" ? "Active" : "Completed" }, { label: "Build", sub: mode === "building" ? "In progress" : mode === "watching" || mode === "failed" ? "Pending" : "Completed" }, { label: "Sign", sub: mode === "signing" ? "In progress" : ["installing", "ready"].includes(mode) ? "Completed" : "Pending" }, { label: "Install", sub: mode === "installing" ? "In progress" : mode === "ready" ? "Completed" : "Pending" }]; const current = mode === "building" ? 1 : mode === "signing" ? 2 : mode === "installing" ? 3 : mode === "ready" ? 4 : 0; return <div className="step-progress">{steps.map((step, index) => <div className="step-wrap" key={step.label}><div className={`step-line ${index < current ? "complete" : ""}`} /><div className={`step ${index < current ? "complete" : index === current ? "current" : "pending"}`}>{index < current ? <FiCheck /> : index === current ? <span /> : <FiCircle />}</div><b>{index + 1} {step.label}</b><small>{step.sub}</small></div>)}</div>; };

const DeviceCard = ({ device, selected, devices, choose }: { device?: DevSyncDevice; selected?: string; devices: DevSyncDevice[]; choose: (id: string) => void }) => <section className="glass-card device-card-dark"><div className="card-heading"><span className="eyebrow">TARGET DEVICE</span><FiChevronDown /></div><div className="device-card-content"><div><label className="device-picker"><select value={selected ?? ""} aria-label="Target device" onChange={(event) => choose(event.target.value)}><option value="">Select iPhone…</option>{devices.map((item) => <option key={item.id} value={item.id}>{item.name}</option>)}</select><FiChevronDown /></label><p className="device-model">{device?.model ?? "Pair an iPhone with this Mac"}{device?.osVersion ? ` · iOS ${device.osVersion}` : ""}</p><p className={device ? "device-connected" : "device-unavailable"}><span className={`status-dot ${device ? "green" : "muted"}`} />{device ? connection(device) : selected ? "Currently unavailable" : "No device selected"}</p></div><DevicePreview model={device?.model} /></div></section>;
const DevicePreview = ({ model }: { model?: string }) => <div className={`device-preview ${model?.toLowerCase().includes("ipad") ? "tablet" : ""}`}><div className="device-screen"><FiSmartphone /></div></div>;
const SigningCard = ({ status, enabled, setBackground }: { status: { label: string; tone: string; detail: string }; enabled: boolean; setBackground: (enabled: boolean) => void }) => <section className="glass-card signing-card-dark"><div className="signing-title"><div><h2>Automatic signing</h2><p>Manage profiles and background sync automatically.</p></div><Toggle checked={enabled} onChange={setBackground} /></div><div className={`signing-status ${status.tone}`}><FiCheckCircle /><div><b>{status.label}</b><span>{status.detail}</span></div></div></section>;

const ActivityCard = ({ workspace }: { workspace: Workspace }) => { const [clearing, setClearing] = useState(false); const activities = [...(workspace.activities ?? [])].reverse().slice(0, 4); const clear = async () => { setClearing(true); try { await devsyncApi.clearActivity(workspace.id); toast.success("Activity cleared"); } catch (error) { toast.error(`Unable to clear activity: ${fail(error)}`); } finally { setClearing(false); } }; return <section className="glass-card activity-card"><div className="card-heading"><span className="eyebrow">ACTIVITY</span><button onClick={clear} disabled={clearing || activities.length === 0}>Clear</button></div>{activities.length ? <div className="activity-list">{activities.map((activity) => <ActivityRow key={activity.id} activity={activity} />)}</div> : <div className="activity-empty"><FiClock /><span>No activity yet</span></div>}</section>; };
const ActivityRow = ({ activity }: { activity: ActivityEntry }) => <div className="activity-row"><span className={`activity-icon ${activity.kind}`}>{activity.kind === "error" ? <FiAlertCircle /> : activity.kind === "build" ? <FiBox /> : activity.kind === "signing" ? <FiShield /> : activity.kind === "install" ? <FiSmartphone /> : <FiCheckCircle />}</span><span>{activity.message}</span><time>{formatTime(activity.timestamp)}</time></div>;

const ProjectDetails = ({ workspace, inspection, building, syncing, inspect, build, remove, refresh }: { workspace: Workspace; inspection?: WorkspaceInspection; building: boolean; syncing: boolean; inspect: (inspection: WorkspaceInspection) => void; build: (workspace: Workspace) => void; remove: (workspace: Workspace) => void; refresh: () => void }) => { const containers = inspection?.containers ?? []; const schemes = inspection?.metadata?.schemes ?? []; const saveContainer = async (path: string) => { const container = containers.find((candidate) => candidate.path === path); if (container) inspect(await devsyncApi.selectContainer(workspace.id, container)); }; const saveScheme = async (scheme: string) => { if (scheme) inspect(await devsyncApi.selectScheme(workspace.id, scheme)); }; const savePreBuild = async (command: string) => { const saved = await devsyncApi.setPreBuildCommand(workspace.id, command); inspect({ workspace: saved, containers, metadata: inspection?.metadata }); }; return <div className="project-details"><div className="detail-grid"><label>Project container<select value={workspace.xcodeContainerPath ?? ""} onChange={(event) => void saveContainer(event.target.value)} disabled={!containers.length}><option value="">{containers.length ? "Select Xcode project…" : "No project detected"}</option>{containers.map((container) => <option key={container.path} value={container.path}>{container.path.split("/").pop()} ({container.containerType})</option>)}</select></label><label>Scheme<select value={workspace.selectedScheme ?? ""} onChange={(event) => void saveScheme(event.target.value)} disabled={!schemes.length}><option value="">{schemes.length ? "Select scheme…" : "No scheme detected"}</option>{schemes.map((scheme) => <option key={scheme} value={scheme}>{scheme}</option>)}</select></label><label className="wide">Pre-Build command<input defaultValue={workspace.preBuildCommand ?? ""} placeholder="Optional, e.g. npm run ios:bundle" onBlur={(event) => void savePreBuild(event.currentTarget.value)} /></label></div><div className="detail-actions"><button className="ghost-button" onClick={refresh}><FiRefreshCw /> Refresh metadata</button><button className="ghost-button" disabled={!isSyncReady(workspace) || building || syncing} onClick={() => build(workspace)}>{building ? <FiLoader className="spin" /> : <FiCode />} Build only</button><button className="danger-button" onClick={() => remove(workspace)}>Remove project</button></div></div>; };

const Devices = ({ devices, selection, choose, refresh }: { devices: DevSyncDevice[]; selection: DeviceSelection; choose: (id: string) => void; refresh: () => void }) => <PageFrame title="Devices" subtitle="Choose the iPhone DevSync uses for deployment." action={<button className="ghost-button" onClick={() => void refresh()}><FiRefreshCw /> Refresh</button>}><section className="page-card device-list-dark">{devices.length ? devices.map((device) => <button className={device.id === selection.selectedDeviceId ? "selected" : ""} key={device.id} onClick={() => void choose(device.id)}><DevicePreview model={device.model} /><span><b>{device.name}</b><small>{device.model ?? "iPhone"} · {connection(device)}</small></span>{device.id === selection.selectedDeviceId && <em>Selected</em>}</button>) : <div className="empty-state"><FiSmartphone /><h2>No iPhone available</h2><p>Pair your iPhone with this Mac, then refresh.</p></div>}</section></PageFrame>;
const Signing = ({ projects, backgroundService, setBackground }: { projects: Workspace[]; backgroundService: BackgroundServiceStatus; setBackground: (enabled: boolean) => void }) => <PageFrame title="Signing" subtitle="DevSync uses the signing already configured in Xcode."><section className="page-card signing-list-dark"><div className="settings-row-dark"><div><b>Automatic signing renewal</b><p>Refresh development profiles before they expire.</p></div><Toggle checked={backgroundService.enabled} onChange={setBackground} /></div>{projects.map((workspace) => { const status = signingSummary(workspace); return <div className="settings-row-dark" key={workspace.id}><div><b>{workspace.productName ?? workspace.displayName}</b><p>{workspace.signingTeam ?? "Team unavailable"}</p></div><span className={`inline-status ${status.tone}`}>{status.label}</span></div>; })}</section></PageFrame>;
const Settings = ({ login, cache, backgroundService, setBackground, setLogin, clean }: { login: boolean; cache?: number; backgroundService: BackgroundServiceStatus; setBackground: (enabled: boolean) => void; setLogin: (enabled: boolean) => void; clean: () => void }) => <PageFrame title="Settings" subtitle="Keep DevSync quietly ready when you need it."><section className="settings-groups-dark"><Group title="General"><SettingRow title="Launch DevSync at Login" text="Keep DevSync ready in the background."><Toggle checked={login} onChange={setLogin} /></SettingRow><SettingRow title="Background Service" text="Run Auto Sync and signing renewal without the window."><Toggle checked={backgroundService.enabled} onChange={setBackground} /></SettingRow></Group><Group title="Build"><SettingRow title="Build Cache" text={cache === undefined ? "Calculating…" : bytes(cache)}><button className="ghost-button" disabled={!cache} onClick={clean}>Clean</button></SettingRow></Group><Group title="Advanced"><SettingRow title="Signing renewal threshold" text="DevSync checks signing automatically."><span className="setting-value">24 hours</span></SettingRow></Group></section></PageFrame>;
const PageFrame = ({ title, subtitle, action, children }: { title: string; subtitle: string; action?: ReactNode; children: ReactNode }) => <><header className="subpage-header"><div><span className="eyebrow">DEVSYNC</span><h1>{title}</h1><p>{subtitle}</p></div>{action}</header>{children}</>;
const Group = ({ title, children }: { title: string; children: ReactNode }) => <section className="settings-group"><span className="eyebrow">{title}</span><div>{children}</div></section>;
const SettingRow = ({ title, text, children }: { title: string; text: string; children: ReactNode }) => <div className="settings-row-dark"><div><b>{title}</b><p>{text}</p></div>{children}</div>;
const Toggle = ({ checked, onChange }: { checked: boolean; onChange: (value: boolean) => void }) => <label className="toggle"><input type="checkbox" checked={checked} onChange={(event) => onChange(event.target.checked)} /><i /></label>;

function deriveState(workspace: Workspace, update: DeploymentUpdate | undefined, building: boolean, syncing: boolean, device: DevSyncDevice | undefined): { mode: AuroraMode; title: string; description: string; header: string } {
  const state = update?.state ?? workspace.deploymentState; const target = device?.name ?? "your iPhone";
  if (state === "buildFailed" || state === "installFailed" || workspace.metadataError || workspace.signingStatus?.status === "expired") return { mode: "failed", title: "Needs attention", description: workspace.metadataError ?? workspace.deploymentMessage ?? "Review the project before syncing.", header: "Attention needed" };
  if (state === "waitingForDevice") return { mode: "waiting", title: "Waiting", description: `Connect ${target} to continue…`, header: "Waiting for iPhone…" };
  if (state === "installing") return { mode: "installing", title: "Installing", description: `Installing the latest build on ${target}…`, header: "Installing on iPhone…" };
  if (state === "signing") return { mode: "signing", title: "Signing", description: `Preparing a signed build for ${target}…`, header: "Signing build…" };
  if (state === "building" || state === "preBuilding" || state === "preBuildSucceeded" || building) return { mode: "building", title: "Building", description: "Compiling the latest project changes…", header: "Building project…" };
  if (state === "preparing" || syncing) return { mode: "watching", title: "Preparing", description: "Getting the project ready to sync…", header: "Preparing sync…" };
  if (state === "installed" && !workspace.changesDetected) return { mode: "ready", title: "Ready", description: `The latest build is on ${target}.`, header: "Watching for changes…" };
  if (workspace.changesDetected) return { mode: "watching", title: "Watching", description: "Changes detected. Ready to build and install.", header: "Changes detected…" };
  return { mode: "watching", title: "Watching", description: "DevSync is listening for project changes.", header: "Watching for changes…" };
}
function emptyState() { return { mode: "watching" as AuroraMode, title: "Watching", description: "Add a project to begin.", header: "Waiting for a project…" }; }
function isSyncReady(workspace: Workspace) { return !workspace.unavailable && !workspace.metadataError && !!(workspace.xcodeContainerPath && workspace.selectedScheme); }
function stateLabel(state: DeploymentState) { return ({ idle: "Ready", preparing: "Preparing", preBuilding: "Pre-Build", preBuildSucceeded: "Pre-Build complete", building: "Building", buildFailed: "Build failed", buildSucceeded: "Build complete", signing: "Signing", waitingForDevice: "Waiting for iPhone", installing: "Installing", installFailed: "Install failed", installed: "Completed" })[state]; }
function footerStatus(workspace: Workspace, device?: DevSyncDevice) { if (workspace.metadataError) return "Project needs attention"; if (workspace.changesDetected) return "Changes detected"; if (device) return "DevSync is ready"; return "Waiting for iPhone"; }
function signingSummary(workspace: Workspace) { const status = workspace.signingStatus; if (!status) return { label: "No profile inspected", tone: "muted", detail: "Build a project to inspect signing." }; if (status.status === "expired") return { label: "Signing expired", tone: "error", detail: "Sync a fresh signed build to continue." }; if (status.status === "unknown") return { label: "Signing unavailable", tone: "muted", detail: "Build a project to inspect signing." }; if (status.status === "expiringSoon") return { label: "Signing expires soon", tone: "warning", detail: formatRemaining(status.remainingSeconds) }; return { label: "Signing available", tone: "good", detail: formatRemaining(status.remainingSeconds) }; }
function formatRemaining(seconds?: number) { if (seconds === undefined) return "Expiration time unavailable"; if (seconds <= 0) return "Expired"; const days = Math.floor(seconds / 86400); const hours = Math.floor((seconds % 86400) / 3600); return days ? `Valid for ${days}d ${hours}h` : `Valid for ${hours}h`; }
function compactPath(path: string) { const parts = path.split("/").filter(Boolean); return parts.length > 3 ? `~/${parts.slice(-2).join("/")}` : path.replace(/^\/Users\/[^/]+/, "~"); }
function connection(device: DevSyncDevice) { return device.connectionState.trim().toLowerCase() === "unknown" ? "Checking device…" : `${device.connectionState}${device.connectionType ? ` over ${device.connectionType}` : ""}`; }
function isConnected(device: DevSyncDevice) { return ["connected", "available", "online"].includes(device.connectionState.trim().toLowerCase()); }
function formatTime(value: string) { const date = new Date(value); return Number.isNaN(date.getTime()) ? "—" : date.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" }); }
function bytes(value: number) { return value < 1048576 ? `${Math.ceil(value / 1024)} KB` : `${(value / 1048576).toFixed(1)} MB`; }
