use config::{Config, File, FileFormat};
use log::info;
use rustx::connect_message::ConnectMessage;
use rustx::fixed_header::FixedHeader;
use rustx::puback_message::PubAckMessage;
use rustx::publish_message::PublishMessage;
use rustx::suback_message::SubAckMessage;
use rustx::subscribe_message::SubscribeMessage;
use rustx::subscribe_return_code::SubscribeReturnCode;
use std::collections::HashMap;
use std::io::{Error, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

/// Procesa los mensajes entrantes de cada cliente.
fn handle_client(mut stream: TcpStream) -> Result<(), Error> {
    // Lee el inicio de un mensaje, para determinar su tipo
    const FIXED_HEADER_LEN: usize = FixedHeader::fixed_header_len();
    let mut fixed_header_buf: [u8; 2] = [0; FIXED_HEADER_LEN];

    let _res = stream.read(&mut fixed_header_buf)?;

    // He leído bytes de un fixed_header, tengo que ver de qué tipo es.
    let fixed_header = FixedHeader::from_bytes(fixed_header_buf.to_vec());
    let tipo = fixed_header.get_tipo();
    /*println!(
        "Recibo msj con tipo: {}, bytes de fixed header leidos: {:?}",
        tipo, fixed_header
    );*/
    // El único tipo válido es el de connect, xq siempre se debe iniciar la comunicación con un connect.
    match tipo {
        1 => {
            // Es tipo Connect, acá procesar el connect y dar connack
            println!("Recibo mensaje tipo Connect");
            let msg_bytes =
                continuar_leyendo_bytes_del_msg(fixed_header, &mut stream, &fixed_header_buf)?;
            // Entonces tengo el mensaje completo
            let connect_msg = ConnectMessage::from_bytes(&msg_bytes);
            println!(
                "   Mensaje connect completo recibido: \n   {:?}",
                connect_msg
            );

            // Se fija si la conexión se establece correctamente y responde un connack
            let username = "sistema-monitoreo";
            let password = "rutx123";
            let is_authentic: bool = msg_bytes
                .windows(username.len() + password.len() + 2)
                .any(|slice| slice == [username.as_bytes(), password.as_bytes(), &[0x00]].concat());

            let connack_response: [u8; 4] = if is_authentic {
                [0x20, 0x02, 0x00, 0x00] // CONNACK (0x20) con retorno 0x00
            } else {
                [0x20, 0x02, 0x00, 0x05] // CONNACK (0x20) con retorno 0x05 (Refused, not authorized)
            };
            let _ = stream
                .write(&connack_response)
                .expect("Error al enviar CONNACK");
            stream.flush()?;
            println!(
                "   tipo connect: Enviado el ack: \n   {:?}",
                connack_response
            );

            //if is_authentic { // ToDo: actuamente está dando siempre false, fijarse.
            // A partir de ahora que ya se hizo el connect exitosamente,
            // se puede empezar a recibir publish y subscribe de ese cliente.
            // Acá: un mismo cliente puede enviarme muchos mensajes, no solamente uno.
            // Por ello, acá debe haber un loop. Leo, y le paso lo leído a la función
            // hasta que lea [0, 0].
            let mut fixed_header_buf = leer_fixed_header_de_stream_y_obt_tipo(&mut stream)?;
            //let mut n = fixed_header.is_not_null(); // move occurs
            let ceros: &[u8; 2] = &[0; 2];
            let mut vacio = &fixed_header_buf == ceros;
            while !vacio {
                println!("Adentro del while");
                continuar_la_conexion(&mut stream, fixed_header_buf)?; // esta función lee UN mensaje.
                                                                       // Leo para la siguiente iteración
                fixed_header_buf = leer_fixed_header_de_stream_y_obt_tipo(&mut stream)?;
                vacio = &fixed_header_buf == ceros;
            }

            //}; // (fin del if es authentic, está comentado pero debe conservarse).
        }
        _ => {
            println!("Error, el primer mensaje recibido DEBE ser un connect.");
            // ToDo: Leer de la doc qué hacer en este caso, o si solo ignoramos.
        }
    };
    Ok(())
}

/// Se ejecuta una vez recibido un `ConnectMessage` exitoso y devuelto un `ConnAckMessage` acorde.
/// Se puede empezar a recibir mensajes de otros tipos (`Publish`, `Subscribe`), de este cliente.
/// Recibe el `stream` para la comunicación con el cliente en cuestión.
/// Lee un mensaje.
fn continuar_la_conexion(stream: &mut TcpStream, fixed_header_bytes: [u8; 2]) -> Result<(), Error> {
    // He leído bytes de un fixed_header, tengo que ver de qué tipo es.
    let fixed_header = FixedHeader::from_bytes(fixed_header_bytes.to_vec());
    let tipo = fixed_header.get_tipo();
    println!("--------------------------");
    println!(
        "Recibo fixed header, tipo: {}, bytes de fixed header leidos: {:?}",
        tipo, fixed_header
    );
    // Ahora sí ya puede haber diferentes tipos de mensaje.
    match tipo {
        3 => {
            println!("Recibo mensaje tipo Publish");
            let msg_bytes =
                continuar_leyendo_bytes_del_msg(fixed_header, stream, &fixed_header_bytes)?;
            // Entonces tengo el mensaje completo
            let msg = PublishMessage::from_bytes(msg_bytes)?;
            println!("   Mensaje publish completo recibido: {:?}", msg);

            // Ahora tengo que mandarle un PubAck
            let option_packet_id = msg.get_packet_identifier();
            let packet_id = option_packet_id.unwrap_or(0);
            // ToDo: Acá imagino que hago algún checkeo, leer la doc
            let ack = PubAckMessage::new(packet_id, 0);
            let msg_bytes = ack.to_bytes();
            let _ = stream.write(&msg_bytes)?;
            stream.flush()?;
            println!("   tipo publish: Enviado el ack: \n   {:?}", ack);

            // Les tengo que enviar mensaje Publish a quienes se suscribieron a su topic
            let topic = msg.get_topic();
            // Tengo que buscar, para ese topic, todos sus subscribers, los guardé cuando hicieron subscribe


        }
        8 => {
            // Acá  análogo, para cuando recibo un Subscribe
            println!("Recibo mensaje tipo Subscribe");
            let msg_bytes =
                continuar_leyendo_bytes_del_msg(fixed_header, stream, &fixed_header_bytes)?;
            // Entonces tengo el mensaje completo
            let msg = SubscribeMessage::from_bytes(msg_bytes)?;
            //println!("   Mensaje completo recibido: {:?}", msg);

            // Proceso lo recibido
            // Tengo una lista de subscribers por cada topic, y guardo en ella al cliente
            // para después mandarle lo que se publique en ese topic
            //aux: let subs_by_topic = Hashmap<topic, subscriber>
            let mut subs_by_topic: HashMap<String, Vec<&mut TcpStream>> = HashMap::new(); // le pondría arc mutex, pero es único hilo por ahora []
            //let mut subs_by_topic: HashMap<String, Vec<Arc<&mut TcpStream>>> = HashMap::new(); // le pongo arc xq si no rust no me deja x tema de borrows
            // ^ aux, obviamente este hashmap se crea afuera.
            
            let mut return_codes = vec![];
            for (topic, _qos) in msg.get_topic_filters() {
                return_codes.push(SubscribeReturnCode::QoS1); // [] ToDo: ver bien qué mandarle
                
                // Agrego el stream del cliente que se está suscribiendo, a la lista de suscriptores
                // para ese topic, en el hashmap, para poder enviarle lo que se publique en dicho tpoic                
                if let Ok(stream_cl) = stream.try_clone() { // <-- [] rust no me deja, voy a tener que hacer arc.
                    let topic_s = topic.to_string();
                    subs_by_topic.entry(topic_s)
                                .or_insert_with(|| Vec::new())
                                .push(stream)
                }

            }

            // Le mando un SubAck. ToDo: leer bien los campos a mandar.
            let ack = SubAckMessage::new(0, return_codes);
            let msg_bytes = ack.to_bytes();
            let _ = stream.write(&msg_bytes)?;
            stream.flush()?;
            println!("   tipo subscribe: Enviado el ack: \n   {:?}", ack);
        }
        _ => println!(
            "   ERROR: tipo desconocido: recibido: \n   {:?}",
            fixed_header
        ),
    };

    Ok(())
}

/// Lee `fixed_header` bytes del `stream`, sabe cuántos son por ser de tamaño fijo el fixed_header.
/// Determina el tipo del mensaje recibido que inicia por `fixed_header`.
/// Devuelve el tipo, y por cuestiones de optimización (ahorrar conversiones)
/// devuelve también fixed_header (el struct encabezado del mensaje) y fixed_header_buf (sus bytes).
//fn leer_fixed_header_de_stream_y_obt_tipo(stream: &mut TcpStream) -> Result<(u8, [u8; 2], FixedHeader), Error> {
fn leer_fixed_header_de_stream_y_obt_tipo(stream: &mut TcpStream) -> Result<[u8; 2], Error> {
    // Leer un fixed header y obtener tipo
    const FIXED_HEADER_LEN: usize = FixedHeader::fixed_header_len();
    let mut fixed_header_buf: [u8; 2] = [0; FIXED_HEADER_LEN];

    let _res = stream.read(&mut fixed_header_buf)?;

    // He leído bytes de un fixed_header, tengo que ver de qué tipo es.
    //let fixed_header = FixedHeader::from_bytes(fixed_header_buf.to_vec());
    //let tipo = fixed_header.get_tipo();

    //return Ok((tipo, fixed_header_buf, fixed_header));
    Ok(fixed_header_buf)
}

/// Una vez leídos los dos bytes del fixed header de un mensaje desde el stream,
/// lee los siguientes `remaining length` bytes indicados en el fixed header.
/// Concatena ambos grupos de bytes leídos para conformar los bytes totales del mensaje leído.
/// (Podría hacer fixed_header.to_bytes(), se aprovecha que ya se leyó fixed_header_bytes).
fn continuar_leyendo_bytes_del_msg(
    fixed_header: FixedHeader,
    stream: &mut TcpStream,
    fixed_header_bytes: &[u8; 2],
) -> Result<Vec<u8>, Error> {
    // Instancio un buffer para leer los bytes restantes, siguientes a los de fixed header
    let msg_rem_len: usize = fixed_header.get_rem_len();
    let mut rem_buf = vec![0; msg_rem_len];
    let _res = stream.read(&mut rem_buf)?;

    // Ahora junto las dos partes leídas, para obt mi msg original
    let mut buf = fixed_header_bytes.to_vec();
    buf.extend(rem_buf);
    println!(
        "   Mensaje en bytes recibido, antes de hacerle from_bytes: {:?}",
        buf
    );
    Ok(buf)
}

fn main() -> Result<(), Error> {
    env_logger::init();

    info!("Leyendo archivo de configuración.");
    let mut config = Config::default();
    config
        .merge(File::new(
            "message_broker_server_config.properties",
            FileFormat::Toml,
        ))
        .unwrap();

    let ip = config
        .get::<String>("ip")
        .unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = config.get::<u16>("port").unwrap_or(9090);
    let listener =
        TcpListener::bind(format!("{}:{}", ip, port)).expect("Error al enlazar el puerto");

    println!("Servidor iniciado. Esperando conexiones.");
    let mut handles = vec![];

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let handle = std::thread::spawn(|| {
                    let _ = handle_client(stream);
                });
                handles.push(handle);
            }
            Err(e) => {
                println!("Error al aceptar la conexión: {}", e);
            }
        }
    }

    for handle in handles {
        if let Err(e) = handle.join() {
            eprintln!("Error al esperar el hilo: {:?}", e);
        }
    }

    Ok(())
}
