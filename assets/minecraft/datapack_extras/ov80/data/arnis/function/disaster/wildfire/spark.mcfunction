execute store result entity @s Rotation[0] float 1 run random value 0..359
execute store result storage arnis:ds sc.d int 1 run random value 0..9
function arnis:disaster/wildfire/spark_at with storage arnis:ds sc
kill @s
