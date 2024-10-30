FROM ubuntu:latest
LABEL authors="Guy Corbaz"
WORKDIR /usr/local/opcgw

# Building opcgw
#RUN cargo build --release
COPY ./target/release/opc_ua_chirpstack_gateway .
COPY log4rs.yaml .
COPY config ./config
EXPOSE 4855
#RUN useradd opcgw
#USER opcgw
CMD ["./opc_ua_chirpstack_gateway"]
#ENTRYPOINT ["opc_ua_chirpstack_gateway"]