# A player right-clicked some entity: if it was a car, they get in. Sneaking gets them out.
advancement revoke @s only arnis:cars/board
tag @s add arnis.boarding
execute as @e[type=interaction,tag=arnis.car.part,distance=..12] if data entity @s interaction run function arnis:cars/board_car
execute as @e[type=interaction,tag=arnis.tp,tag=!arnis.tdeck,distance=..12] if data entity @s interaction run function arnis:trains/board
# A boat's deck is as wide as the boat, so it is clicked from well off its middle.
execute as @e[type=interaction,tag=arnis.tdeck,distance=..48] if data entity @s interaction run function arnis:trains/board
tag @s remove arnis.boarding
