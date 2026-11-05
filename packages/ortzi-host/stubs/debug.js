const noop = () => {};
const debug = () => Object.assign(noop, { enabled: false, extend: debug });
debug.enable = noop;
debug.disable = noop;
debug.enabled = () => false;
export default debug;
