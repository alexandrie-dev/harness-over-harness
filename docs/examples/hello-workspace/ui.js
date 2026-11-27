export function mount(root) {
  const heading = document.createElement('h1');
  const button = document.createElement('button');
  let clicks = 0;

  Object.assign(root.style, {
    padding: '24px',
    font: '16px system-ui',
    colorScheme: 'light dark',
  });
  heading.textContent = 'Hello Workspace';
  button.type = 'button';
  button.textContent = 'Clicks: 0';
  button.addEventListener('click', () => {
    button.textContent = `Clicks: ${++clicks}`;
  });
  root.append(heading, button);
}
