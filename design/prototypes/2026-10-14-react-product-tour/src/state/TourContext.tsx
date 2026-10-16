import {
  createContext,
  useContext,
  useEffect,
  useReducer,
  useState,
  type ReactNode,
} from "react";
import { reducer } from "../domain/reducer";
import { current, currentScene, findReference } from "../domain/selectors";
import type { Command, ModalState, TourState, UiState } from "../domain/types";
import { initialState } from "../domain/seed";
import { loadState, saveState } from "./persistence";
interface TourContextValue {
  state: TourState;
  scene: ReturnType<typeof currentScene>;
  data: ReturnType<typeof current>;
  dispatch: (command: Command) => void;
  commit: (command: Command) => boolean;
  setUi: (patch: Partial<UiState>) => void;
  modal: ModalState;
  setModal: (modal: ModalState) => void;
  reference: (id: string) => void;
  selectScene: (id: string) => void;
  sidebarOpen: boolean;
  setSidebarOpen: (open: boolean) => void;
}
const Context = createContext<TourContextValue | null>(null);
export interface TourProviderProps {
  children: ReactNode;
  storageKey: string;
  persist: boolean;
  initialChapter: string;
  migrateLegacy: boolean;
}
export function TourProvider({
  children,
  storageKey,
  persist,
  initialChapter,
  migrateLegacy,
}: TourProviderProps) {
  const [state, dispatch] = useReducer(reducer, null, () =>
    initialState(true, initialChapter),
  );
  const hydrationKey = [
    storageKey,
    persist,
    initialChapter,
    migrateLegacy,
  ].join(":");
  const [loadedKey, setLoadedKey] = useState<string | null>(null);
  const [modal, setModal] = useState<ModalState>(null),
    [sidebarOpen, setSidebarOpen] = useState(false);
  useEffect(() => {
    dispatch({
      type: "restore",
      state: persist
        ? loadState(
            storageKey,
            window.innerWidth >= 1000,
            initialChapter,
            migrateLegacy,
          )
        : initialState(window.innerWidth >= 1000, initialChapter),
    });
    setLoadedKey(hydrationKey);
  }, [storageKey, persist, initialChapter, migrateLegacy]);
  useEffect(() => {
    if (persist && loadedKey === hydrationKey) saveState(storageKey, state);
  }, [state, storageKey, persist, loadedKey, hydrationKey]);
  useEffect(() => {
    if (!state.notice) return;
    const timer = setTimeout(() => dispatch({ type: "dismissNotice" }), 3500);
    return () => clearTimeout(timer);
  }, [state.notice]);
  const setUi = (patch: Partial<UiState>) => dispatch({ type: "ui", patch });
  const commit = (command: Command) => {
    const next = reducer(state, command);
    dispatch(command);
    return next.cache !== state.cache;
  };
  const selectScene = (id: string) => {
    dispatch({ type: "selectScene", id });
    setModal(null);
    setSidebarOpen(false);
  };
  const reference = (id: string) => {
    const ref = findReference(state, id);
    if (!ref) {
      dispatch({ type: "notice", text: "这个引用已经不可用。" });
      return;
    }
    if (ref.sceneId !== state.sceneId)
      dispatch({ type: "selectScene", id: ref.sceneId });
    if (ref.convId !== state.cache[ref.sceneId].ui.conv)
      dispatch({ type: "ui", patch: { conv: ref.convId } });
    if (ref.kind === "plan" || ref.kind === "ask" || ref.kind === "request")
      setModal({ kind: ref.kind, id });
    else {
      setModal(null);
      dispatch({
        type: "ui",
        patch: {
          tab: "document",
          doc: id,
          selectedFolders: {
            ...state.cache[ref.sceneId].ui.selectedFolders,
            document: "all",
          },
        },
      });
    }
  };
  return (
    <Context.Provider
      value={{
        state,
        scene: currentScene(state),
        data: current(state),
        dispatch,
        commit,
        setUi,
        modal,
        setModal,
        reference,
        selectScene,
        sidebarOpen,
        setSidebarOpen,
      }}
    >
      {children}
    </Context.Provider>
  );
}
export function useTour() {
  const value = useContext(Context);
  if (!value) throw Error("PendingCrewTour components require TourProvider");
  return value;
}
