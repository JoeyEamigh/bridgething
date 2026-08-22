import './global.css';

import { createNativeBottomTabNavigator } from '@bottom-tabs/react-navigation';
import { createBottomTabNavigator } from '@react-navigation/bottom-tabs';
import { NavigationContainer } from '@react-navigation/native';
import {
  createNativeStackNavigator,
  type NativeStackNavigationOptions,
} from '@react-navigation/native-stack';
import { PortalHost } from '@rn-primitives/portal';
import { useEffect, useMemo, useState } from 'react';
import { ActivityIndicator, StatusBar, View } from 'react-native';
import { SafeAreaProvider } from 'react-native-safe-area-context';

import { CrashBoundary } from './components/CrashBoundary';
import { StatusLine } from './components/StatusLine';
import { TabBar } from './components/TabBar';
import { Wordmark } from './components/Wordmark';
import { refreshCatalog, startWebappAutoUpdate } from './lib/catalog';
import { startReachability } from './lib/reachability';
import { bootstrapSession } from './lib/session';
import { getNativeTabs, getSetupCompleted } from './lib/storage';
import { navTheme, PALETTE, usePalette, useScheme } from './lib/theme';
import type {
  AppsStackParamList,
  RootStackParamList,
  SettingsStackParamList,
  StoreStackParamList,
  TabParamList,
} from './navigation';
import { AppsScreen } from './screens/Apps';
import { DebugScreen } from './screens/Debug';
import { DeviceDetailScreen } from './screens/DeviceDetail';
import { LogsScreen } from './screens/Logs';
import { OtaVersionsScreen } from './screens/OtaVersions';
import { SettingsScreen } from './screens/Settings';
import { SetupScreen } from './screens/Setup';
import { StoreScreen } from './screens/Store';
import { StoreAppScreen } from './screens/StoreApp';
import { StoreSourceScreen } from './screens/StoreSource';
import { StoreSourcesScreen } from './screens/StoreSources';
import { WebappDetailScreen } from './screens/WebappDetail';
import { WebappSettingsScreen } from './screens/WebappSettings';
import { WebappSlotsScreen } from './screens/WebappSlots';

const RootStack = createNativeStackNavigator<RootStackParamList>();
const Tabs = createBottomTabNavigator<TabParamList>();
const NativeTabs = createNativeBottomTabNavigator<TabParamList>();
const AppsStack = createNativeStackNavigator<AppsStackParamList>();
const StoreStack = createNativeStackNavigator<StoreStackParamList>();
const SettingsStack = createNativeStackNavigator<SettingsStackParamList>();

type BootRoute = 'Tabs' | 'Setup';

function useStackOptions(): NativeStackNavigationOptions {
  const palette = usePalette();
  return useMemo(
    () => ({
      headerStyle: { backgroundColor: palette.bg },
      headerTintColor: palette.fg,
      headerShadowVisible: false,
      headerTitleStyle: {
        fontWeight: '700',
        fontSize: 17,
        letterSpacing: -0.2,
      },
      contentStyle: { backgroundColor: palette.bg },
      headerBackButtonDisplayMode: 'minimal',
    }),
    [palette],
  );
}

function AppsTab() {
  const options = useStackOptions();
  return (
    <AppsStack.Navigator screenOptions={options}>
      <AppsStack.Screen
        name="Apps"
        component={AppsScreen}
        options={{ headerTitle: () => <Wordmark size="sm" /> }}
      />
      <AppsStack.Screen
        name="DeviceDetail"
        component={DeviceDetailScreen}
        options={{ title: '' }}
      />
      <AppsStack.Screen
        name="OtaVersions"
        component={OtaVersionsScreen}
        options={{ title: 'choose version' }}
      />
      <AppsStack.Screen
        name="WebappDetail"
        component={WebappDetailScreen}
        options={{ title: '' }}
      />
      <AppsStack.Screen
        name="WebappSettings"
        component={WebappSettingsScreen}
      />
      <AppsStack.Screen
        name="WebappSlots"
        component={WebappSlotsScreen}
        options={{ title: 'home screen' }}
      />
    </AppsStack.Navigator>
  );
}

function StoreTab() {
  const options = useStackOptions();
  return (
    <StoreStack.Navigator screenOptions={options}>
      <StoreStack.Screen
        name="Store"
        component={StoreScreen}
        options={{ headerShown: false }}
      />
      <StoreStack.Screen
        name="StoreSources"
        component={StoreSourcesScreen}
        options={{ title: 'sources' }}
      />
      <StoreStack.Screen
        name="StoreSource"
        component={StoreSourceScreen}
        options={{ title: 'source' }}
      />
      <StoreStack.Screen
        name="StoreApp"
        component={StoreAppScreen}
        options={{ title: '' }}
      />
    </StoreStack.Navigator>
  );
}

function SettingsTab() {
  const options = useStackOptions();
  return (
    <SettingsStack.Navigator screenOptions={options}>
      <SettingsStack.Screen
        name="Settings"
        component={SettingsScreen}
        options={{ headerShown: false }}
      />
      <SettingsStack.Screen
        name="Logs"
        component={LogsScreen}
        options={{ title: 'logs' }}
      />
      <SettingsStack.Screen
        name="Debug"
        component={DebugScreen}
        options={{ title: 'debug' }}
      />
    </SettingsStack.Navigator>
  );
}

function TabsScreen() {
  const palette = usePalette();

  if (getNativeTabs()) {
    return (
      <View className="flex-1">
        <NativeTabs.Navigator
          tabBarActiveTintColor={palette.accent}
          tabBarInactiveTintColor={palette.soft}
          tabBarStyle={{ backgroundColor: palette.bg }}
          rippleColor={palette.accentSoft}
          hapticFeedbackEnabled
        >
          <NativeTabs.Screen
            name="apps"
            component={AppsTab}
            options={{
              title: 'apps',
              tabBarIcon: () => ({ sfSymbol: 'square.grid.2x2' }),
            }}
          />
          <NativeTabs.Screen
            name="store"
            component={StoreTab}
            options={{
              title: 'store',
              tabBarIcon: () => ({ sfSymbol: 'storefront' }),
            }}
          />
          <NativeTabs.Screen
            name="settings"
            component={SettingsTab}
            options={{
              title: 'settings',
              tabBarIcon: () => ({ sfSymbol: 'gearshape' }),
            }}
          />
        </NativeTabs.Navigator>
        <StatusLine floating />
      </View>
    );
  }

  return (
    <Tabs.Navigator
      tabBar={props => <TabBar {...props} />}
      screenOptions={{
        headerShown: false,
        sceneStyle: { backgroundColor: palette.bg },
      }}
    >
      <Tabs.Screen name="apps" component={AppsTab} />
      <Tabs.Screen name="store" component={StoreTab} />
      <Tabs.Screen name="settings" component={SettingsTab} />
    </Tabs.Navigator>
  );
}

export default function App() {
  const scheme = useScheme();
  const palette = PALETTE[scheme];
  const barStyle = scheme === 'dark' ? 'light-content' : 'dark-content';

  const [boot, setBoot] = useState<BootRoute | null>(null);

  useEffect(() => {
    let cancelled = false;
    setBoot(getSetupCompleted() ? 'Tabs' : 'Setup');
    startReachability();
    startWebappAutoUpdate();
    bootstrapSession().catch(err => {
      if (cancelled) return;
      console.warn('[bridgething] bootstrap failed', err);
    });
    refreshCatalog().catch(err => {
      if (cancelled) return;
      console.warn('[bridgething] catalog refresh failed', err);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  if (boot == null) {
    return (
      <SafeAreaProvider>
        <StatusBar barStyle={barStyle} backgroundColor={palette.bg} />
        <View
          className="flex-1 items-center justify-center"
          style={{ backgroundColor: palette.bg }}
        >
          <Wordmark size="lg" />
          <View className="mt-8">
            <ActivityIndicator size="small" color={palette.accent} />
          </View>
        </View>
      </SafeAreaProvider>
    );
  }

  return (
    <SafeAreaProvider>
      <StatusBar barStyle={barStyle} backgroundColor={palette.bg} />
      <CrashBoundary>
        <NavigationContainer theme={navTheme[scheme]}>
          <RootStack.Navigator
            initialRouteName={boot}
            screenOptions={{ headerShown: false }}
          >
            <RootStack.Screen name="Setup" component={SetupScreen} />
            <RootStack.Screen name="Tabs" component={TabsScreen} />
          </RootStack.Navigator>
        </NavigationContainer>
      </CrashBoundary>
      <PortalHost />
    </SafeAreaProvider>
  );
}
