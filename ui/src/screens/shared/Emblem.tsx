// Эмблема Alexandrie (утверждённая эмблема сайта, копия в src/assets, фон сделан прозрачным только в копии).
// Единственное место знака: шапка мастера и левый верхний угол окна берут его отсюда.
import emblem1x from '../../assets/emblem-24.png';
import emblem2x from '../../assets/emblem-48.png';

export const EMBLEM_SOURCE = emblem2x;

export function Emblem({ size = 24 }: { size?: number }) {
  return <img className="emblem" data-emblem src={emblem1x} srcSet={`${emblem1x} 1x, ${emblem2x} 2x`} alt="" aria-hidden="true" draggable={false}
    style={{ blockSize: size, inlineSize: 'auto' }} />;
}
