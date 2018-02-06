extern crate nanomsg;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::sync::mpsc::{channel, Sender, Receiver};
use std::thread;
use std::time::Duration;

use nanomsg::{Protocol, Socket};
use serde_json::{Map, Value};

const BASE_URL: &'static str = "ipc:///tmp";
const ADAPTER_MANAGER_URL: &'static str = "ipc:///tmp/gateway.addonManager";

#[derive(Serialize)]
#[serde(tag = "messageType", content = "data", rename_all = "camelCase")]
enum PluginRegisterMessage {
    #[serde(rename_all = "camelCase")]
    RegisterPlugin {
        plugin_id: String,
    },
}

#[derive(Deserialize)]
#[serde(tag = "messageType", content = "data", rename_all = "camelCase")]
enum GatewayRegisterMessage {
    #[serde(rename_all = "camelCase")]
    RegisterPluginReply {
        plugin_id: String,
        ipc_base_addr: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "messageType", content = "data", rename_all = "camelCase")]
enum GatewayMessage {
    #[serde(rename_all = "camelCase")]
    UnloadPlugin {
        plugin_id: String,
    },
    #[serde(rename_all = "camelCase")]
    UnloadAdapter {
        plugin_id: String,
        adapter_id: String,
    },

    #[serde(rename_all = "camelCase")]
    SetProperty {
        plugin_id: String,
        adapter_id: String,
        device_id: String,
        property: Property,
    },
    #[serde(rename_all = "camelCase")]
    StartPairing {
        plugin_id: String,
        adapter_id: String,
        timeout: f64,
    },
    #[serde(rename_all = "camelCase")]
    CancelPairing {
        plugin_id: String,
        adapter_id: String,
    },
    #[serde(rename_all = "camelCase")]
    RemoveThing {
        plugin_id: String,
        adapter_id: String,
        device_id: String,
    },
    #[serde(rename_all = "camelCase")]
    CancelRemoveThing {
        plugin_id: String,
        adapter_id: String,
        device_id: String,
    },
}

#[derive(Serialize)]
#[serde(tag = "messageType", content = "data", rename_all = "camelCase")]
enum PluginMessage {
    #[serde(rename_all = "camelCase")]
    PluginUnloaded {
        plugin_id: String,
    },
    #[serde(rename_all = "camelCase")]
    AdapterUnloaded {
        plugin_id: String,
        adapter_id: String,
    },

    #[serde(rename_all = "camelCase")]
    AddAdapter {
        plugin_id: String,
        adapter_id: String,
        name: String,
    },
    #[serde(rename_all = "camelCase")]
    HandleDeviceAdded {
        plugin_id: String,
        adapter_id: String,
        id: String,
        name: String,
        typ: String,
        properties: Map<String, Value>,
        actions: Map<String, Value>,
    },
    #[serde(rename_all = "camelCase")]
    HandleDeviceRemoved {
        plugin_id: String,
        adapter_id: String,
        id: String,
    },
    #[serde(rename_all = "camelCase")]
    PropertyChanged {
        plugin_id: String,
        adapter_id: String,
        device_id: String,
        property: Property,
    },
}

#[derive(Debug, Deserialize, Serialize)]
struct Property {
    name: String,
    value: Value,
}

struct GatewayBridge {
    id: String,
    msg_sender: Sender<GatewayMessage>,
    msg_receiver: Receiver<PluginMessage>
}

impl GatewayBridge {
    fn new(id: &str) -> (GatewayBridge, Sender<PluginMessage>, Receiver<GatewayMessage>) {
        let (gp_sender, gp_receiver) = channel();
        let (pg_sender, pg_receiver) = channel();
        (
            GatewayBridge {
                id: id.to_string(),
                msg_sender: gp_sender,
                msg_receiver: pg_receiver,
            },
            pg_sender,
            gp_receiver
        )
    }

    fn run_forever(&mut self) -> Result<(), io::Error> {
        let mut socket = Socket::new(Protocol::Req)?;
        let mut endpoint = socket.connect(ADAPTER_MANAGER_URL)?;
        let req = PluginRegisterMessage::RegisterPlugin {
            plugin_id: self.id.to_string()
        };
        socket.write_all(serde_json::to_string(&req)?.as_bytes())?;
        let mut rep = String::new();
        socket.read_to_string(&mut rep)?;
        endpoint.shutdown()?;
        println!("We got it! {}", rep);
        let msg: GatewayRegisterMessage = serde_json::from_str(&rep)?;
        // open a Req channel to adapterManager
        // send {messageType: 'registerPlugin', data: { pluginId: id }}
        // receives
        // {
        //  messageType: 'registerPluginReply',
        //  data: {
        //    pluginId: 'pluginId-string',
        //    ipcBaseAddr: 'gateway.plugin.xxx',
        //  },
        //}
        // connect to ipcBaseAddr as pair
        // then handle everything

        let ipc_base_addr = match msg {
            GatewayRegisterMessage::RegisterPluginReply {ipc_base_addr, ..} => {
                ipc_base_addr
            },
        };

        let mut socket_pair = Socket::new(Protocol::Pair)?;
        let mut endpoint_pair = socket_pair.connect(&format!("{}/{}", BASE_URL, &ipc_base_addr))?;

        let mut buf = Vec::new();

        loop {
            let read_status = socket.nb_read_to_end(&mut buf);
            if read_status.is_ok() {
                match serde_json::from_slice(&buf) {
                    Ok(msg) => {
                        self.msg_sender.send(msg).unwrap();
                    },
                    _ => {
                    }
                }
            }

            if let Ok(msg_to_send) = self.msg_receiver.try_recv() {
                socket_pair.write_all(serde_json::to_string(&msg_to_send)?.as_bytes()).unwrap();
                match msg_to_send {
                    PluginMessage::PluginUnloaded {..} => {
                        println!("run_forever exiting");
                        endpoint_pair.shutdown()?;
                        return Ok(());
                    }
                    _ => {}
                }
            }

            thread::sleep(Duration::from_millis(33));
        }
    }
}

fn to_io_error<E>(err: E) -> io::Error
    where E: Into<Box<std::error::Error+Send+Sync>> {
    io::Error::new(io::ErrorKind::Other, err)
}

struct Device {
    id: String,
    props: HashMap<String, Value>
}

impl Device {
    fn new(id: &str) -> Device {
        Device {
            id: id.to_string(),
            props: HashMap::new()
        }
    }
}

struct Adapter {
    id: String,
    devices: HashMap<String, Device>
}

struct Plugin {
    id: String,
    adapters: HashMap<String, Adapter>,
    sender: Sender<PluginMessage>,
    receiver: Receiver<GatewayMessage>,
}

impl Plugin {
    fn new(id: &str, sender: Sender<PluginMessage>, receiver: Receiver<GatewayMessage>) -> Plugin {
        Plugin {
            id: id.to_string(),
            sender: sender,
            receiver: receiver,
            adapters: HashMap::new(),
        }
    }

    fn handle_msg(&self, msg: GatewayMessage) -> Result<(), io::Error> {
        match msg {
            GatewayMessage::SetProperty {
                plugin_id,
                adapter_id,
                device_id,
                property
            } => {
                if plugin_id != self.id {
                    return Ok(())
                }

                let adapter = &self.adapters[&adapter_id];
                adapter.set_property(device_id, property)
            },
            GatewayMessage::UnloadPlugin {..} => {
                Ok(())
            },
            GatewayMessage::UnloadAdapter {..} => {
                Ok(())
            },
            GatewayMessage::StartPairing {
                plugin_id,
                adapter_id,
                timeout: _,
            } => {
                if plugin_id != self.id {
                    return Ok(())
                }

                let adapter = &self.adapters[&adapter_id];
                adapter.start_pairing()
            },
            GatewayMessage::CancelPairing {
                plugin_id,
                adapter_id,
            } => {
                if plugin_id != self.id {
                    return Ok(())
                }

                let adapter = &self.adapters[&adapter_id];
                adapter.cancel_pairing()
            },
            GatewayMessage::RemoveThing { .. } => {
                Ok(())
            },
            GatewayMessage::CancelRemoveThing { .. } => {
                Ok(())
            }
        }
    }

    fn run_forever(&mut self) -> Result<(), io::Error> {
        loop {
            match self.receiver.try_recv() {
                Ok(msg) => {
                    println!("recv: {:?}", msg);
                    self.handle_msg(msg)?;
                },
                _ => {}
            }
        }
    }
}

fn main() {
    let (mut gateway_bridge, msg_sender, msg_receiver) = GatewayBridge::new("mqtt");
    thread::spawn(move || {
        gateway_bridge.run_forever().unwrap();
    });
    let mut plugin = Plugin::new("mqtt", msg_sender, msg_receiver);
    plugin.run_forever().unwrap();

    // let adapters = map from id to adapter
    // select (nanomsg, paired bridges channel)
    // send a start/cancel pairing to the bridge proc if requested
    // dispatch commands to the addapters list
    // let light_id = "1";

    // let props = LightProperties {
    //     on: true,
    //     hue: 0,
    //     sat: 0,
    //     bri: 255
    // };
    // let _ = adapters[0].send_properties(light_id, props).unwrap();
}