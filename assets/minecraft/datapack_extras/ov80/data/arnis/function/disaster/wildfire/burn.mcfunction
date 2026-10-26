function arnis:disaster/util/follow_ground
fill ~-6 ~-2 ~-6 ~6 ~5 ~6 minecraft:fire replace #arnis:kindling
execute summon marker run function arnis:disaster/wildfire/spark
execute summon marker run function arnis:disaster/wildfire/spark
execute summon marker run function arnis:disaster/wildfire/spark
execute summon marker run function arnis:disaster/wildfire/spark
execute summon marker run function arnis:disaster/wildfire/spark
playsound minecraft:block.fire.ambient ambient @a[distance=..32] ~ ~ ~ 2 0.8
