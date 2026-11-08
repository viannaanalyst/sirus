#!/usr/bin/env ruby
# Adds the Live Activity extension to the generated iOS project, and turns Live Activities on in
# the app's Info.plist. `ios/` isn't tracked, so run it again after a prebuild. Safe to repeat.
# Usage: GEM_PATH=<cocoapods gems> ruby scripts/add-live-activity-target.rb
require 'xcodeproj'

mobile = File.expand_path('..', __dir__)
project_path = File.join(mobile, 'ios/Sirus.xcodeproj')
project = Xcodeproj::Project.open(project_path)
name = 'SirusLiveActivity'
exit 0 if project.targets.any? { |target| target.name == name }

app = project.targets.find { |target| target.name == 'Sirus' }
team = app.build_configurations.first.build_settings['DEVELOPMENT_TEAM']
target = project.new_target(:app_extension, name, :ios, '16.4')

group = project.main_group.new_group(name, '../targets/live-activity')
target.add_file_references([group.new_file('SirusAgentActivity.swift')])
group.new_file('Info.plist')

target.build_configurations.each do |config|
  config.build_settings.merge!(
    'PRODUCT_NAME' => '$(TARGET_NAME)',
    'PRODUCT_BUNDLE_IDENTIFIER' => 'com.siruscode.phone.activity',
    'INFOPLIST_FILE' => '../targets/live-activity/Info.plist',
    'GENERATE_INFOPLIST_FILE' => 'NO',
    'DEVELOPMENT_TEAM' => team,
    'CODE_SIGN_STYLE' => 'Automatic',
    'SWIFT_VERSION' => '5.0',
    'MARKETING_VERSION' => '1.0',
    'CURRENT_PROJECT_VERSION' => '1',
    'TARGETED_DEVICE_FAMILY' => '1',
    'SKIP_INSTALL' => 'YES',
    'LD_RUNPATH_SEARCH_PATHS' => '$(inherited) @executable_path/Frameworks @executable_path/../../Frameworks'
  )
end

embed = app.new_copy_files_build_phase('Embed Foundation Extensions')
embed.symbol_dst_subfolder_spec = :plug_ins
embed.add_file_reference(target.product_reference).settings = { 'ATTRIBUTES' => ['RemoveHeadersOnInstall'] }
app.add_dependency(target)
project.save

plist = File.join(mobile, 'ios/Sirus/Info.plist')
system('/usr/libexec/PlistBuddy', '-c', 'Add :NSSupportsLiveActivities bool true', plist, err: File::NULL)
