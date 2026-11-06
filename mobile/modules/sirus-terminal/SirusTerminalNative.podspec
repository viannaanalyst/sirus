require 'json'

package = JSON.parse(File.read(File.join(__dir__, 'package.json')))

Pod::Spec.new do |s|
  s.name = 'SirusTerminalNative'
  s.version = package['version']
  s.summary = 'Native terminal surface for the Sirus iPhone app.'
  s.description = 'A libghostty terminal surface for the Sirus React Native app.'
  s.homepage = 'https://github.com/viannaanalyst/sirus'
  s.license = { :type => 'MIT' }
  s.author = 'Sirus'
  s.platforms = { :ios => '16.1' }
  s.source = { :path => '.' }
  s.source_files = 'ios/**/*.{h,m,mm,swift}'
  s.vendored_frameworks = 'Vendor/libghostty/GhosttyKit.xcframework'
  s.frameworks = 'IOSurface', 'Metal', 'MetalKit', 'QuartzCore', 'UIKit'
  s.libraries = 'c++', 'z'
  s.swift_version = '5.9'
  s.dependency 'ExpoModulesCore'
end
