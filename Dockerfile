#
# Docker file for opc ua chirpstack gateway
#

FROM ubuntu
LABEL authors="Guy Corbaz"
RUN apt-get update && apt-get install -y iputils-ping
# Define work folder
WORKDIR /usr/local/opcgw

# Copy necessary files
# However, configuration should be
# on a permanent storage
COPY ./target/release/opcgw .
COPY log4rs.yaml .
COPY config ./config

RUN ls -al /usr/local/opcgw

RUN chmod +x /usr/local/opcgw/opcgw

EXPOSE 4855
RUN useradd opcgw
USER opcgw
ENTRYPOINT ["./opcgw"]

