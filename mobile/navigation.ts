import type { BrowseQuery } from '@bridgething/catalog';
import type { BottomTabScreenProps } from '@react-navigation/bottom-tabs';
import type {
  CompositeScreenProps,
  NavigationProp,
  NavigatorScreenParams,
} from '@react-navigation/native';
import type { NativeStackScreenProps } from '@react-navigation/native-stack';

export type AppsStackParamList = {
  Apps: undefined;
  DeviceDetail: { deviceId: string };
  OtaVersions: { deviceId: string; channel: string };
  WebappDetail: { deviceId: string; id: string };
  WebappSettings: { deviceId: string; id: string; name: string };
  WebappSlots: { deviceId: string };
};

export type StoreStackParamList = {
  Store: undefined;
  StoreSources: { deviceId: string | null } | undefined;
  StoreSource: { deviceId: string | null; url: string; name: string };
  StoreApp: { deviceId: string | null; appId: string; sourceUrl: string };
  StoreBrowse: { deviceId: string | null; query: BrowseQuery };
};

export type SettingsStackParamList = {
  Settings: undefined;
  Logs: undefined;
  Debug: undefined;
};

export type TabParamList = {
  apps: NavigatorScreenParams<AppsStackParamList> | undefined;
  store: NavigatorScreenParams<StoreStackParamList> | undefined;
  settings: NavigatorScreenParams<SettingsStackParamList> | undefined;
};

export type RootStackParamList = {
  Setup: { step?: number } | undefined;
  Tabs: NavigatorScreenParams<TabParamList> | undefined;
};

export type TabName = keyof TabParamList;

export type RootNavigation = NavigationProp<RootStackParamList>;

type TabScreenProps<T extends TabName> = CompositeScreenProps<
  BottomTabScreenProps<TabParamList, T>,
  NativeStackScreenProps<RootStackParamList>
>;

export type RootScreenProps<T extends keyof RootStackParamList> =
  NativeStackScreenProps<RootStackParamList, T>;

export type AppsScreenProps<T extends keyof AppsStackParamList> =
  CompositeScreenProps<
    NativeStackScreenProps<AppsStackParamList, T>,
    TabScreenProps<'apps'>
  >;

export type StoreScreenProps<T extends keyof StoreStackParamList> =
  CompositeScreenProps<
    NativeStackScreenProps<StoreStackParamList, T>,
    TabScreenProps<'store'>
  >;

export type SettingsScreenProps<T extends keyof SettingsStackParamList> =
  CompositeScreenProps<
    NativeStackScreenProps<SettingsStackParamList, T>,
    TabScreenProps<'settings'>
  >;
