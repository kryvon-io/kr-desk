import { MemoryBus, MemoryHub } from "../src/bus.js";
import { MemoryStore } from "../src/store.js";
import { signalingSuite } from "./signaling.suite.js";

signalingSuite("sinalização (bus em memória)", () => {
  const hub = new MemoryHub();
  return { makeBus: () => new MemoryBus(hub), makeStore: () => new MemoryStore() };
});
