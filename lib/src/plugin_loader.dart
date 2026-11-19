import 'dart:io';

import 'host_runtime.dart';

/// Multi-plugin manager: loads N plugin instances and records the RSS
/// after each load for the memory-curve verdict.
class PluginLoader {
  final List<GrainPlugin> _plugins = [];
  final List<(int count, int rssBytes)> rssSamples = [];

  List<GrainPlugin> get plugins => List.unmodifiable(_plugins);

  static int currentRss() => ProcessInfo.currentRss;

  Future<GrainPlugin> load(String path) async {
    final plugin = await GrainPlugin.instantiate(path);
    _plugins.add(plugin);
    return plugin;
  }

  /// Loads plugins until [target] instances exist, sampling RSS after each.
  /// Re-instantiates the same module to simulate N distinct plugins.
  Future<void> loadUpTo(String path, int target) async {
    while (_plugins.length < target) {
      await load(path);
      rssSamples.add((_plugins.length, currentRss()));
    }
  }

  void disposeAll() {
    for (final plugin in _plugins) {
      plugin.dispose();
    }
    _plugins.clear();
  }
}
