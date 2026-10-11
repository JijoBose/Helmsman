# Loaded only into Kamal processes Helmsman starts (RUBYOPT=-r).
#
# Kamal 2.12 stores --skip-hooks on the parent Thor command. The subcommand
# still sees the default false, so pre-connect and other hooks run. Reporting
# every hook as missing keeps those processes read-only.
trace = TracePoint.new(:end) do |event|
  defined = event.self
  next unless defined.is_a?(Module) && defined.name == "Kamal::Commands::Hook"

  defined.prepend(Module.new do
    def hook_exists?(*)
      false
    end
  end)
  event.disable
end
trace.enable
