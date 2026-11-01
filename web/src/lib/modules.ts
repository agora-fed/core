export function isModuleOff(moduleId: string, root?: { dataset: DOMStringMap }): boolean {
  const el = root ?? (typeof document === 'undefined' ? undefined : document.documentElement);
  return (el?.dataset.modulesOff ?? '').split(/\s+/).includes(moduleId);
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
