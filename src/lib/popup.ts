/** Desktop draws some menus in a native popup. Here every menu stays in the page. */
export const popupOpen = (): string | null => null
export const justClosed = (_menu: string) => false
export const closeMenu = () => {}
export const openMenu = async (..._args: unknown[]) => {}
export const onPopupChange = (_fn: (open: string | null) => void) => () => {}
