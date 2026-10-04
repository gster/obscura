// Captured and deleted by the host before author code. This initializer only
// accepts host-captured callbacks and the original document capability.
(() => {
  'use strict';
  const apply = Reflect.apply;
  const create = Object.create;
  const define = Object.defineProperty;
  const freeze = Object.freeze;
  const has = WeakMap.prototype.has;
  const get = WeakMap.prototype.get;
  const set = WeakMap.prototype.set;
  const mapGet = Map.prototype.get;
  const mapSet = Map.prototype.set;
  const mapDelete = Map.prototype.delete;
  const then = Promise.prototype.then;
  const ArrayCtor = Array;
  const WeakMapCtor = WeakMap;
  const MapCtor = Map;
  const TypeErrorCtor = TypeError;
  const EventPrototype = Event.prototype;
  const AbortSignalRemove = AbortSignal.prototype.removeEventListener;
  const EventTargetCtor = EventTarget;
  const EventTargetPrototype = EventTarget.prototype;
  const ObjectPrototype = Object.prototype;
  const primitives = globalThis.__obscura_speech_primitives;
  if (!primitives || !delete globalThis.__obscura_speech_primitives) {
    throw new TypeErrorCtor('Speech primitives handoff failed');
  }
  const listeners = primitives.listeners;
  const markTrusted = primitives.markTrusted;
  const now = primitives.now;
  const enqueueDelivery = primitives.enqueueDelivery;
  const setServiceRegistry = primitives.setServiceRegistry;
  const reportError = console.error;
  const reportReceiver = console;
  const windowObject = globalThis;
  const toStringTag = Symbol.toStringTag;

  const read = (map, key) => apply(get, map, [key]);
  const write = (map, key, value) => apply(set, map, [key, value]);
  const contains = (map, key) => apply(has, map, [key]);
  const readType = (map, type) => apply(mapGet, map, [type]);
  const own = (object, key, value) => {
    const descriptor = create(null);
    descriptor.value = value;
    descriptor.writable = true;
    descriptor.enumerable = true;
    descriptor.configurable = true;
    return define(object, key, descriptor);
  };
  const put = own;
  const copy = values => {
    const result = new ArrayCtor(values.length);
    for (let i = 0; i < values.length; i++) put(result, i, values[i]);
    return result;
  };
  const report = error => { try { apply(reportError, reportReceiver, [error]); } catch (_) {} };

  // Use the engine's Event state and trusted-event registry, bypassing public
  // constructor/dispatch properties that an author can replace while awaiting
  // the provider. This event is created only for an actual provider delivery.
  const voiceEvent = service => {
    const event = create(EventPrototype);
    own(event, 'type', 'voiceschanged');
    own(event, 'bubbles', false);
    own(event, 'cancelable', false);
    own(event, 'composed', false);
    own(event, 'defaultPrevented', false);
    own(event, 'target', service);
    own(event, 'currentTarget', service);
    own(event, 'eventPhase', 2);
    own(event, 'timeStamp', now());
    own(event, '_propagationStopped', false);
    own(event, '_immediatePropagationStopped', false);
    markTrusted(event);
    return event;
  };

  const listenerIndex = (values, entry) => {
    for (let i = 0; i < values.length; i++) if (values[i] === entry) return i;
    return -1;
  };
  const removeEntry = (service, entry) => {
    const types = read(listeners, service);
    const entries = types && readType(types, 'voiceschanged');
    if (!entries) return;
    const i = listenerIndex(entries, entry);
    if (i >= 0) {
      for (let j = i + 1; j < entries.length; j++) put(entries, j - 1, entries[j]);
      delete entries[entries.length - 1];
      entries.length--;
    }
    if (!entries.length) apply(mapDelete, types, ['voiceschanged']);
    if (entry.signal && entry.abortHandler) {
      try { apply(AbortSignalRemove, entry.signal, ['abort', entry.abortHandler]); }
      catch (error) { report(error); }
      entry.abortHandler = null;
    }
  };
  const dispatchDelivery = service => {
    const event = voiceEvent(service);
    const types = read(listeners, service);
    const entries = types && readType(types, 'voiceschanged');
    if (entries) {
      const snapshot = copy(entries);
      for (let i = 0; i < snapshot.length; i++) {
        const entry = snapshot[i];
        const current = readType(types, 'voiceschanged');
        if (!current || listenerIndex(current, entry) < 0) continue;
        if (entry.once) removeEntry(service, entry);
        try {
          if (typeof entry.callback === 'function') apply(entry.callback, service, [event]);
          else {
            const callback = entry.callback.handleEvent;
            if (typeof callback === 'function') apply(callback, entry.callback, [event]);
          }
        } catch (error) { report(error); }
        if (event._immediatePropagationStopped) break;
      }
    }
    try { own(event, 'currentTarget', null); own(event, 'eventPhase', 0); } catch (error) { report(error); }
  };

  const initialize = configuration => {
    const { nativeOwnerActive, nativeRequestInventory, sharedRegistry } = configuration;
    if (typeof nativeOwnerActive !== 'function' || typeof nativeRequestInventory !== 'function') {
      throw new TypeErrorCtor('Speech host callbacks are required');
    }
    const registry = sharedRegistry || freeze({
      services: new WeakMapCtor(), voices: new WeakMapCtor(), windows: new WeakMapCtor(),
    });
    setServiceRegistry(registry.services);
    const servicePrototype = create(EventTargetPrototype);
    const voicePrototype = create(ObjectPrototype);
    const tag = (prototype, value) => {
      const descriptor = create(null);
      descriptor.value = value;
      descriptor.configurable = true;
      define(prototype, toStringTag, descriptor);
    };
    tag(servicePrototype, 'SpeechSynthesis');
    tag(voicePrototype, 'SpeechSynthesisVoice');
    const service = create(servicePrototype);
    const record = {
      service, voices: [], requested: false, handler: null, handlerEntry: null,
      active: nativeOwnerActive, request: nativeRequestInventory,
      eventListeners: listeners, ensureInventory: null, setHandler: null,
      // Blink builds a sequence in the receiver's relevant realm.
      copyVoices: values => {
        const result = new ArrayCtor(values.length);
        for (let i = 0; i < values.length; i++) put(result, i, values[i]);
        return result;
      },
    };
    write(registry.services, service, record);
    write(registry.windows, windowObject, record);

    const serviceRecord = receiver => {
      const result = read(registry.services, receiver);
      if (!result) throw new TypeErrorCtor('Illegal invocation');
      return result;
    };
    const voiceRecord = receiver => {
      const result = read(registry.voices, receiver);
      if (!result) throw new TypeErrorCtor('Illegal invocation');
      return result;
    };
    const deliver = values => {
      if (!record.active()) return;
      // Each real OnSetVoiceList delivery replaces all wrappers, including an
      // identical or empty list. Earlier wrappers retain their scalar metadata.
      const voices = new ArrayCtor(values.length);
      for (let i = 0; i < values.length; i++) {
        const metadata = values[i];
        const voice = create(voicePrototype);
        write(registry.voices, voice, freeze({
          voiceURI: metadata.voiceURI, name: metadata.name, lang: metadata.lang,
          localService: metadata.localService, default: metadata.default,
        }));
        put(voices, i, voice);
      }
      record.voices = voices;
      dispatchDelivery(service);
    };
    const requestNext = revision => {
      if (!record.active()) return;
      try {
        const pending = record.request(revision);
        // Each actual async op resolves one delivery. Never wait for default
        // completion before publishing the already available initial snapshot.
        const descriptor = create(null);
        descriptor.value = undefined;
        define(pending, 'constructor', descriptor);
        apply(then, pending, [delivery => {
          if (!record.active()) return;
          // Inventory events are browser tasks; a Promise reaction alone does
          // not end another event's IndexedDB transaction activity period.
          enqueueDelivery(() => {
            if (!record.active()) return;
            if (delivery.voices !== null) deliver(delivery.voices);
            // Terminal-only completion never manufactures a voiceschanged event.
            if (!delivery.done && record.active()) requestNext(delivery.revision);
          });
        }, error => { report(error); }]);
      } catch (error) { report(error); }
    };
    record.ensureInventory = () => {
      if (record.requested || !record.active()) return;
      record.requested = true;
      requestNext(-1);
    };

    function getVoices() {
      const receiver = serviceRecord(this);
      receiver.ensureInventory();
      return receiver.copyVoices(receiver.voices);
    }
    function windowGet() {
      const receiver = read(registry.windows, this);
      if (!receiver) throw new TypeErrorCtor('Illegal invocation');
      receiver.ensureInventory();
      return receiver.service;
    }
    function onvoiceschangedGet() { return serviceRecord(this).handler; }
    record.setHandler = value => {
      const receiver = record;
      // EventHandler stores objects but only invokes callable functions. This
      // deliberately differs from addEventListener's handleEvent callbacks.
      const handler = value !== null && (typeof value === 'object' || typeof value === 'function') ? value : null;
      receiver.handler = handler;
      if (handler !== null && !receiver.handlerEntry) {
        const wrapper = function(event) {
          const current = receiver.handler;
          if (typeof current === 'function') apply(current, receiver.service, [event]);
        };
        const entry = { callback: wrapper, capture: false, once: false, passive: false, signal: null, abortHandler: null };
        receiver.handlerEntry = entry;
        let types = read(listeners, receiver.service);
        if (!types) { types = new MapCtor(); write(listeners, receiver.service, types); }
        let entries = readType(types, 'voiceschanged');
        if (!entries) { entries = []; apply(mapSet, types, ['voiceschanged', entries]); }
        put(entries, entries.length, entry);
      } else if (handler === null && receiver.handlerEntry) {
        removeEntry(receiver.service, receiver.handlerEntry);
        receiver.handlerEntry = null;
      }
    };
    function onvoiceschangedSet(value) { serviceRecord(this).setHandler(value); }
    function voiceURI() { return voiceRecord(this).voiceURI; }
    function name() { return voiceRecord(this).name; }
    function lang() { return voiceRecord(this).lang; }
    function localService() { return voiceRecord(this).localService; }
    function defaultGetter() { return voiceRecord(this).default; }

    return freeze({
      registry, service, servicePrototype, voicePrototype,
      serviceConstructorParent: EventTargetCtor,
      serviceBrand: receiver => contains(registry.services, receiver),
      voiceBrand: receiver => contains(registry.voices, receiver),
      windowBrand: receiver => contains(registry.windows, receiver),
      getVoices, windowGet, onvoiceschangedGet, onvoiceschangedSet,
      voiceURI, name, lang, localService, default: defaultGetter,
    });
  };
  define(globalThis, '__obscura_speech_handoff', {
    value: initialize, configurable: true, writable: false, enumerable: false,
  });
})();
