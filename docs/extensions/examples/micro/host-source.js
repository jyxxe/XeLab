const input = JSON.parse(stdin.readAll());
let text = input.text;
for (const item of host.extensionData('text.rules')) {
  if (item.config.enabled && item.contribution.operation === 'uppercase-ascii') {
    text = text.replace(/[a-z]/g, c => String.fromCharCode(c.charCodeAt(0) - 32));
  }
}
stdout.write(JSON.stringify({text}));
