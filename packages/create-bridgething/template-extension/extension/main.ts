import { asJson, defineExtension, json } from '@bridgething/extension';

defineExtension({
  start(ctx) {
    ctx.log.info(`__PROJECT_NAME__ extension up, data in ${ctx.dataDir}`);

    ctx.on('device', event => {
      if (event.type !== 'connected') return;
      const greeting = ctx.config(event.device).greeting ?? 'hello';
      ctx.log.info(`${event.device.name} connected; greeting is "${greeting}"`);
      event.device.send(json({ type: 'greeting', text: greeting }));
    });

    ctx.on('message', (device, message) => {
      const payload = asJson<Record<string, unknown>>(message);
      if (!payload) return;
      device.send(json({ type: 'echo', payload }));
    });
  },
});
