import { AppProvider, useApp } from './state/app';
import { Backdrop, GlassLayer, TipLayer } from './components/Glass';
import { TitleBar } from './components/TitleBar';
import { Toasts } from './components/Toast';
import { Sheets } from './components/Sheets';
import { Home } from './views/Home';
import { Studio } from './views/Studio';
import { ImageLab } from './views/ImageLab';
import { Access } from './views/Access';

function Shell() {
  const { state } = useApp();
  return (
    <>
      <Backdrop />
      <GlassLayer>
        <div className="ring">
          <div className="slab">
            <TitleBar />
            <div className="view">
              {state.route === 'home' ? <Home /> : null}
              {state.route === 'studio' ? <Studio /> : null}
              {state.route === 'lab' ? <ImageLab /> : null}
              {state.route === 'access' ? <Access /> : null}
            </div>
          </div>
        </div>
      </GlassLayer>
      <Sheets />
      <Toasts />
      <TipLayer />
    </>
  );
}

export function App() {
  return (
    <AppProvider>
      <Shell />
    </AppProvider>
  );
}
