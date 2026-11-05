using System.IO.Pipes;
using System.Security.AccessControl;
using System.Security.Principal;
using System.Text;

namespace LhmBridge;

/// The named pipe. Whatever ends the session, every fan is handed back before `Run` returns (S5, S6).
public static class PipeHost
{
    public const int MaxLineBytes = 64 * 1024;

    public static PipeSecurity SystemOnly()
    {
        var security = new PipeSecurity();
        security.SetAccessRuleProtection(isProtected: true, preserveInheritance: false);
        security.AddAccessRule(new PipeAccessRule(
            new SecurityIdentifier(WellKnownSidType.LocalSystemSid, null),
            PipeAccessRights.ReadWrite | PipeAccessRights.CreateNewInstance | PipeAccessRights.Synchronize,
            AccessControlType.Allow));
        return security;
    }

    /// One client at a time, and refused if the name is taken, so nobody can sit on the pipe first.
    public static NamedPipeServerStream Create(string name, PipeSecurity security) =>
        NamedPipeServerStreamAcl.Create(
            name,
            PipeDirection.InOut,
            maxNumberOfServerInstances: 1,
            PipeTransmissionMode.Byte,
            PipeOptions.Asynchronous | PipeOptions.FirstPipeInstance,
            MaxLineBytes,
            MaxLineBytes,
            security);

    public static void Run(NamedPipeServerStream server, Bridge bridge, TimeSpan watchdogTick, CancellationToken cancel)
    {
        using var stop = CancellationTokenSource.CreateLinkedTokenSource(cancel);
        var watchdog = Task.Run(async () =>
        {
            while (!stop.IsCancellationRequested)
            {
                if (bridge.CheckWatchdog())
                {
                    stop.Cancel();
                    return;
                }
                try
                {
                    await Task.Delay(watchdogTick, stop.Token);
                }
                catch (OperationCanceledException)
                {
                    return;
                }
            }
        });

        try
        {
            server.WaitForConnectionAsync(stop.Token).GetAwaiter().GetResult();
            Serve(server, new Protocol(bridge), stop.Token);
        }
        catch (Exception e) when (e is OperationCanceledException or IOException or InvalidDataException)
        {
            Console.Error.WriteLine($"session ended: {e.Message}");
        }
        finally
        {
            stop.Cancel();
            bridge.RestoreAll();
            server.Dispose();
            watchdog.GetAwaiter().GetResult();
        }
    }

    private static void Serve(NamedPipeServerStream pipe, Protocol protocol, CancellationToken stop)
    {
        var line = new List<byte>();
        var buffer = new byte[4096];
        while (true)
        {
            var count = pipe.ReadAsync(buffer, stop).AsTask().GetAwaiter().GetResult();
            if (count == 0) return; // the client hung up
            for (var i = 0; i < count; i++)
            {
                if (buffer[i] != (byte)'\n')
                {
                    line.Add(buffer[i]);
                    if (line.Count > MaxLineBytes) throw new InvalidDataException("request line too long");
                    continue;
                }
                var reply = protocol.Handle(Encoding.UTF8.GetString(line.ToArray()));
                line.Clear();
                pipe.WriteAsync(Encoding.UTF8.GetBytes(reply + "\n"), stop).AsTask().GetAwaiter().GetResult();
                pipe.FlushAsync(stop).GetAwaiter().GetResult();
                if (protocol.ShutdownRequested) return;
            }
        }
    }
}
