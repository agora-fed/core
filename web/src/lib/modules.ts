/** Off unless the gateway lists them; must match UI_OPTIONAL_MODULES in the gateway. */
const OPTIONAL_MODULES = ['groups'];

const listed = (value: string | undefined, moduleId: string) =>
  (value ?? '').split(/\s+/).includes(moduleId);

export function isModuleOff(moduleId: string, root?: { dataset: DOMStringMap }): boolean {
  const el = root ?? (typeof document === 'undefined' ? undefined : document.documentElement);
  if (OPTIONAL_MODULES.includes(moduleId)) return !listed(el?.dataset.modulesOn, moduleId);
  return listed(el?.dataset.modulesOff, moduleId);
}

export interface ModuleAware {
  requires?: string;
  whenOff?: string;
}

/** Same rule the stylesheet applies, for lists rendered from JS such as the command palette. */
export function visibleForModules<T extends ModuleAware>(
  items: T[],
  isOff: (moduleId: string) => boolean = isModuleOff,
): T[] {
  return items.filter(
    (it) => !(it.requires && isOff(it.requires)) && !(it.whenOff && !isOff(it.whenOff)),
  );
}

export interface ModuleState {
  id: string;
  title: string;
  active: boolean;
  core: boolean;
  gateable: boolean;
}

export const POLITICAL_PACKAGE = [
  'mandates',
  'consequence',
  'scorecard',
  'accountability',
  'proposals',
] as const;

const inPackage = (id: string) => (POLITICAL_PACKAGE as readonly string[]).includes(id);

export const moduleFlagKey = (id: string) => `module.${id}`;

/** Package members are managed as one unit; core modules are not offered at all. */
export function toggleableModules(modules: ModuleState[]): ModuleState[] {
  return modules.filter((m) => !m.core && m.gateable && !inPackage(m.id));
}

export function politicalPackageState(modules: ModuleState[]) {
  const members = modules.filter((m) => inPackage(m.id));
  const missing = members.filter((m) => !m.active).map((m) => m.id);
  return {
    installed: members.length > 0 && missing.length === 0,
    missing,
  };
}
